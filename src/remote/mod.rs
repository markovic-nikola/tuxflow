pub mod fs;
pub mod tunnel;

use std::path::PathBuf;

/// Where a project's files live. Everything that touches the project
/// filesystem or spawns its processes dispatches on this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectLocation {
    Local(PathBuf),
    /// `host` is an ssh destination (config alias or user@host),
    /// `dir` an absolute path on that host.
    Ssh {
        host: String,
        dir: String,
    },
}

impl ProjectLocation {
    /// Opaque key used wherever projects.toml maps by dir-string.
    /// Local absolute paths never start with "ssh://", so keys are unambiguous.
    pub fn key(&self) -> String {
        match self {
            Self::Local(p) => p.to_string_lossy().into_owned(),
            Self::Ssh { host, dir } => format!("ssh://{host}{dir}"),
        }
    }

    pub fn parse(key: &str) -> Self {
        if let Some(rest) = key.strip_prefix("ssh://") {
            if let Some(slash) = rest.find('/') {
                return Self::Ssh {
                    host: rest[..slash].to_string(),
                    dir: rest[slash..].to_string(),
                };
            }
            // ssh:// with no path — treat the whole rest as host, dir = "/"
            return Self::Ssh {
                host: rest.to_string(),
                dir: "/".to_string(),
            };
        }
        Self::Local(PathBuf::from(key))
    }

    pub fn is_remote(&self) -> bool {
        matches!(self, Self::Ssh { .. })
    }

    pub fn host(&self) -> Option<&str> {
        match self {
            Self::Local(_) => None,
            Self::Ssh { host, .. } => Some(host),
        }
    }

    /// The directory path on the machine that owns the files:
    /// local path string, or the remote absolute path.
    pub fn dir_str(&self) -> String {
        match self {
            Self::Local(p) => p.to_string_lossy().into_owned(),
            Self::Ssh { dir, .. } => dir.clone(),
        }
    }

    /// Last path component, used as the default project name.
    pub fn base_name(&self) -> String {
        let dir = self.dir_str();
        dir.trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("project")
            .to_string()
    }
}

/// Quote a string for POSIX sh: wrap in single quotes, escaping embedded ones.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Directory for ssh ControlMaster sockets.
fn control_dir() -> PathBuf {
    let base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(base).join("tuxflow")
}

/// ssh options shared by every invocation (VTE processes, FS probes, tunnels)
/// so they multiplex over one authenticated connection per host.
pub fn ssh_mux_options() -> Vec<String> {
    let dir = control_dir();
    let _ = std::fs::create_dir_all(&dir);
    vec![
        "-o".into(),
        "ControlMaster=auto".into(),
        "-o".into(),
        format!("ControlPath={}/ssh-%C", dir.to_string_lossy()),
        "-o".into(),
        "ControlPersist=120".into(),
        "-o".into(),
        "ServerAliveInterval=15".into(),
    ]
}

/// Same options as a single shell-ready string (for embedding in a VTE command).
pub fn ssh_mux_options_str() -> String {
    ssh_mux_options()
        .chunks(2)
        .map(|c| format!("{} {}", c[0], sh_quote(&c[1])))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Name of the dedicated tmux server socket (`tmux -L`). A separate socket
/// keeps TuxFlow's sessions and options away from the user's own tmux.
pub const TMUX_SOCKET: &str = "tuxflow";

/// Server options applied (idempotently) on every spawn, before any session
/// exists. `exit-empty off` is load-bearing: without it the server started by
/// the option-setting invocation exits immediately (no sessions yet) and
/// `new-session` would boot a fresh server that never saw these options.
/// The rest: no status bar so the pane looks like a plain terminal, mouse so
/// the wheel scrolls tmux history, set-clipboard for terminals that honour
/// OSC 52 (VTE doesn't — see `fetch_tmux_buffer` for the bridge we use
/// instead), deep history for long-running dev servers. No default-shell
/// needed — tmux
/// always runs shell-commands via /bin/sh, so the inner wrapper is POSIX-safe
/// regardless of the user's login shell.
/// (`\;` survives the remote shell as a literal `;`, chaining tmux commands.)
const TMUX_OPTIONS: &str = "set -g exit-empty off \\; \
     set -g status off \\; set -g mouse on \\; set -g set-clipboard on \\; \
     set -g history-limit 50000 \\; set -g escape-time 10 \\; \
     set -g set-titles on \\; set -g set-titles-string '#{pane_title}'";

/// Deterministic tmux session name for one process of one project. Stable
/// across app restarts so a fresh TuxFlow launch reattaches to sessions left
/// running by the previous one. FNV-1a by hand — std's DefaultHasher isn't
/// guaranteed stable across releases. tmux forbids `.`/`:` in names; the
/// slug keeps only shell-innocuous characters.
pub fn remote_session_name(project_key: &str, process_name: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in project_key.bytes().chain([0u8]).chain(process_name.bytes()) {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    let slug: String = process_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .take(24)
        .collect();
    format!("tf-{}-{:08x}", slug, hash as u32)
}

/// Wrap a process command so it runs on `host` in `remote_dir` with `env`,
/// inside the remote user's login shell (profile sourcing preserved).
/// The result replaces ProcessConfig.command in the local `$SHELL -li -c` argv;
/// `exec` makes the VTE child *be* ssh so PID capture and exit codes keep working.
///
/// The command runs inside a tmux session named `session` on a dedicated
/// server (`tmux -L tuxflow`), so a dropped connection or app quit merely
/// detaches — the process keeps running and the next spawn reattaches
/// (`new-session -A`). The command's exit code is recorded in a per-session
/// exit-file and re-raised by the wrapper, so crash detection sees the real
/// status. Hosts without tmux fall back to running the command directly
/// (no persistence, previous behaviour). `fresh_session` additionally kills
/// any surviving session of that name first — used after an explicit stop,
/// where the fire-and-forget remote kill could race a quick restart.
///
/// When `pidfile` is given, the remote login session's PID is written there
/// before tmux takes over; `remote_kill` uses it to kill the whole remote
/// session explicitly on the no-tmux fallback path.
pub fn wrap_remote_command(
    host: &str,
    remote_dir: &str,
    env: &std::collections::HashMap<String, String>,
    command: &str,
    pidfile: Option<&str>,
    session: &str,
    fresh_session: bool,
) -> String {
    let mut env_prefix = String::new();
    if !env.is_empty() {
        let mut keys: Vec<&String> = env.keys().collect();
        keys.sort();
        env_prefix = format!(
            "env {} ",
            keys.iter()
                .map(|k| sh_quote(&format!("{k}={}", env[*k])))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
    let pid_capture = pidfile
        .map(|f| format!("echo $$ > {} && ", sh_quote(f)))
        .unwrap_or_default();
    // The command through the remote user's login+interactive shell so PATH
    // setup (nvm, cargo) applies — runs inside the tmux pane, or directly on
    // the fallback path. `exec env … shell` (not `env … exec`): exec is a
    // builtin env can't run.
    let login_shell = format!(
        "{env_prefix}\"${{SHELL:-/bin/sh}}\" -lic {}",
        sh_quote(command)
    );
    let dir_q = sh_quote(remote_dir);
    let session_q = sh_quote(session);
    // Exit-code file, namespaced by remote uid at runtime: session names are
    // deterministic, so two users of the same host would otherwise fight over
    // one /tmp path — sticky /tmp makes the loser's writes fail silently and
    // exit codes vanish. `$ef` is expanded by the outer shell, so the inner
    // command (built by string concatenation below) bakes in the same path.
    let ef_assign = format!("ef=\"/tmp/.{session}-$(id -u).exit\"");
    // Pane side: run the command, then record its exit code for the wrapper.
    // tmux runs shell-commands via /bin/sh, so this is POSIX-safe.
    let inner = format!("{}\"$ef\"", sh_quote(&format!("{login_shell}; echo $? > ")));
    let kill_stale = if fresh_session {
        format!("tmux -L {TMUX_SOCKET} kill-session -t {session_q} 2>/dev/null; ")
    } else {
        String::new()
    };
    // After the client exits: the exit-file means the command finished — that
    // code wins. No exit-file means the session is still alive (detach) or
    // the attach itself failed — pass the tmux client's status through so
    // real failures aren't masked as clean exits.
    let remote = format!(
        "cd {dir_q} && {pid_capture}\
         if command -v tmux >/dev/null 2>&1; then \
         {ef_assign}; rm -f \"$ef\"; {kill_stale}\
         tmux -L {TMUX_SOCKET} -f /dev/null start-server \\; {TMUX_OPTIONS}; \
         tmux -L {TMUX_SOCKET} new-session -A -s {session_q} -c {dir_q} {inner}; tst=$?; \
         if [ -f \"$ef\" ]; then exit \"$(cat \"$ef\")\"; else exit \"$tst\"; fi; \
         else exec {login_shell}; fi"
    );
    format!(
        "exec ssh -t {} {} {}",
        ssh_mux_options_str(),
        sh_quote(host),
        sh_quote(&remote)
    )
}

/// Newest tmux paste buffer on `host` — the mouse-selection clipboard
/// bridge. tmux stores mouse selections in its paste buffers, but no
/// released VTE implements OSC 52 (checked through 0.84), so the selection
/// can't reach the local clipboard through the terminal. TuxFlow fetches it
/// after a mouse-up instead. Blocking (one ssh round trip over the warm
/// mux) — call from a worker thread.
pub fn fetch_tmux_buffer(host: &str) -> Option<String> {
    let out = std::process::Command::new("ssh")
        .args(ssh_mux_options())
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"])
        .arg(host)
        .arg("--")
        .arg(format!(
            "tmux -L {TMUX_SOCKET} show-buffer 2>/dev/null; true"
        ))
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    if text.is_empty() { None } else { Some(text) }
}

/// Scrollback of a tmux pane on `host` (joined wrapped lines, last ~2000
/// rows). Used to re-run port/URL detection after reattaching to a live
/// session: the startup banner with the interesting lines has usually
/// scrolled out of the visible screen. Blocking — worker thread only.
pub fn fetch_pane_history(host: &str, session: &str) -> String {
    let out = std::process::Command::new("ssh")
        .args(ssh_mux_options())
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"])
        .arg(host)
        .arg("--")
        .arg(format!(
            "tmux -L {TMUX_SOCKET} capture-pane -pJ -S -2000 -t {} 2>/dev/null; true",
            sh_quote(session)
        ))
        .output();
    match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout).into_owned(),
        Err(_) => String::new(),
    }
}

/// Names of tmux sessions currently alive on `host`'s TuxFlow server —
/// processes still running detached from a previous app run. Blocking (one
/// ssh round trip) — call from a worker thread. Empty on any failure or
/// when the host has no tmux.
pub fn list_live_sessions(host: &str) -> Vec<String> {
    let out = std::process::Command::new("ssh")
        .args(ssh_mux_options())
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"])
        .arg(host)
        .arg("--")
        .arg(format!(
            "tmux -L {TMUX_SOCKET} list-sessions -F '#{{session_name}}' 2>/dev/null; true"
        ))
        .output();
    match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(str::to_string)
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Startup-time token unique to this app run: keeps remote file names from
/// two machines (or app runs) targeting the same host apart, where a bare
/// local PID could collide.
fn run_token() -> u64 {
    use std::sync::LazyLock;
    static RUN_TOKEN: LazyLock<u64> = LazyLock::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
            ^ (std::process::id() as u64)
    });
    *RUN_TOKEN
}

/// Serial for unique per-spawn remote file names within this app run.
fn next_seq() -> u64 {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Unique remote pidfile path for one spawn. Uniqueness (per-run token +
/// serial) makes kill/respawn race-free — a restart never reads the pidfile
/// of the session that replaced the one being killed.
pub fn new_remote_pidfile() -> String {
    format!("/tmp/.tuxflow-{:x}-{}.pid", run_token(), next_seq())
}

/// Fake `xclip` installed into `~/.local/bin` on remote hosts (which are
/// headless — no real clipboard exists). Terminal agents like Claude Code
/// read pasted images by running `xclip -t TARGETS -o` / `-t image/png -o`;
/// the shim serves whatever image TuxFlow last uploaded from the local
/// clipboard, giving the native paste experience ([Image #1] attachment).
/// The freshness window keeps an old screenshot from re-attaching on later,
/// unrelated pastes.
const CLIPBOARD_SHIM: &str = r#"#!/bin/sh
# TuxFlow clipboard shim v1 (auto-installed; safe to delete)
CLIP="$HOME/.cache/tuxflow/clipboard.png"
fresh() {
    [ -f "$CLIP" ] || return 1
    now=$(date +%s); mt=$(stat -c %Y "$CLIP" 2>/dev/null || echo 0)
    [ $((now - mt)) -le 15 ]
}
case "$*" in
    *"-t TARGETS -o"*) fresh && { echo image/png; exit 0; }; exit 1 ;;
    *"-t image/png -o"*) fresh && exec cat "$CLIP"; exit 1 ;;
    *"-o"*) exit 1 ;;
    *) cat >/dev/null 2>&1; exit 0 ;;
esac
"#;

/// Upload PNG bytes from the local clipboard to `host`'s TuxFlow clipboard
/// file, provisioning the `xclip` shim on first use. Returns the absolute
/// remote path (for typing into non-agent terminals). Blocking — call from
/// a worker thread.
pub fn upload_clipboard_image(host: &str, png: &[u8]) -> Result<String, String> {
    use std::io::Write;
    let script = format!(
        "mkdir -p ~/.cache/tuxflow ~/.local/bin && \
         if [ ! -e ~/.local/bin/xclip ]; then \
         printf '%s' {} > ~/.local/bin/xclip && chmod +x ~/.local/bin/xclip; fi && \
         cat > ~/.cache/tuxflow/clipboard.png && \
         echo \"$HOME/.cache/tuxflow/clipboard.png\"",
        sh_quote(CLIPBOARD_SHIM)
    );
    let mut child = std::process::Command::new("ssh")
        .args(ssh_mux_options())
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"])
        .arg(host)
        .arg("--")
        .arg(script)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to run ssh: {e}"))?;
    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(png)
        .map_err(|e| format!("failed to stream image: {e}"))?;
    let out = child
        .wait_with_output()
        .map_err(|e| format!("ssh failed: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Explicitly kill the remote side of a process: the tmux session (when one
/// was used) and the login session recorded in `pidfile` (TERM, then KILL
/// after a grace second — covers the no-tmux fallback and SIGHUP-trappers).
/// Runs fire-and-forget on a worker thread — never blocks the GTK main
/// thread. (On the fallback path, processes that daemonize into a *new*
/// session still escape; with tmux the session kill catches them.)
pub fn remote_kill(host: &str, pidfile: &str, tmux_session: Option<&str>) {
    let tmux_kill = tmux_session
        .map(|s| {
            format!(
                "tmux -L {TMUX_SOCKET} kill-session -t {} 2>/dev/null; ",
                sh_quote(s)
            )
        })
        .unwrap_or_default();
    let script = format!(
        "{tmux_kill}sid=$(cat {p} 2>/dev/null); rm -f {p}; \
         if [ -n \"$sid\" ]; then \
           pkill -TERM -s \"$sid\" 2>/dev/null; sleep 1; \
           pkill -KILL -s \"$sid\" 2>/dev/null; \
         fi; true",
        p = sh_quote(pidfile)
    );
    let host = host.to_string();
    std::thread::spawn(move || {
        let result = std::process::Command::new("ssh")
            .args(ssh_mux_options())
            .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"])
            .arg(&host)
            .arg(&script)
            .output();
        if let Err(e) = result {
            log::warn!("remote kill on {host} failed to run ssh: {e}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_parse_roundtrip_remote() {
        let loc = ProjectLocation::Ssh {
            host: "user@dev-box".into(),
            dir: "/srv/app".into(),
        };
        assert_eq!(loc.key(), "ssh://user@dev-box/srv/app");
        assert_eq!(ProjectLocation::parse(&loc.key()), loc);
    }

    #[test]
    fn key_parse_roundtrip_local() {
        let loc = ProjectLocation::Local(PathBuf::from("/home/me/proj"));
        assert_eq!(loc.key(), "/home/me/proj");
        assert_eq!(ProjectLocation::parse("/home/me/proj"), loc);
    }

    #[test]
    fn base_name_from_remote_dir() {
        let loc = ProjectLocation::parse("ssh://box/srv/app/");
        assert_eq!(loc.base_name(), "app");
    }

    #[test]
    fn sh_quote_escapes_single_quotes() {
        assert_eq!(sh_quote("it's"), r#"'it'\''s'"#);
        assert_eq!(sh_quote("plain"), "'plain'");
    }

    #[test]
    fn wrap_remote_command_shape() {
        let env = std::collections::HashMap::from([("PORT".to_string(), "3000".to_string())]);
        let cmd = wrap_remote_command(
            "box",
            "/srv/app",
            &env,
            "npm run dev",
            None,
            "tf-dev-1",
            false,
        );
        assert!(cmd.starts_with("exec ssh -t "));
        assert!(cmd.contains("'box'"));
        assert!(cmd.contains("/srv/app"));
        assert!(cmd.contains("PORT=3000"));
        assert!(cmd.contains("npm run dev"));
        assert!(!cmd.contains("echo $$"));
        // Persistent tmux session with graceful no-tmux fallback
        assert!(cmd.contains("new-session -A -s"));
        assert!(cmd.contains("command -v tmux"));
        assert!(cmd.contains("else exec"));
        // Exit code is recorded in the uid-namespaced session exit-file
        assert!(cmd.contains("tf-dev-1-$(id -u).exit"));
        // Not a fresh start — no stale-session kill
        assert!(!cmd.contains("kill-session"));
    }

    #[test]
    fn wrap_remote_command_fresh_kills_stale_session() {
        let env = std::collections::HashMap::new();
        let cmd = wrap_remote_command(
            "box",
            "/srv/app",
            &env,
            "npm run dev",
            None,
            "tf-dev-1",
            true,
        );
        assert!(cmd.contains("kill-session -t "));
    }

    #[test]
    fn wrap_remote_command_with_pidfile() {
        let env = std::collections::HashMap::new();
        let cmd = wrap_remote_command(
            "box",
            "/srv/app",
            &env,
            "npm run dev",
            Some("/tmp/x.pid"),
            "tf-dev-1",
            false,
        );
        // PID capture runs in the login session before tmux takes over
        assert!(cmd.contains("echo $$ > "));
        assert!(cmd.contains("x.pid"));
    }

    #[test]
    fn session_names_are_stable_sanitized_and_project_scoped() {
        let a = remote_session_name("ssh://box/srv/app", "npm: dev.server");
        assert_eq!(
            a,
            remote_session_name("ssh://box/srv/app", "npm: dev.server")
        );
        // tmux forbids '.' and ':' in session names
        assert!(!a.contains('.') && !a.contains(':') && !a.contains(' '));
        assert!(a.starts_with("tf-npm--dev-server-"));
        // Same process name in another project gets a different session
        let b = remote_session_name("ssh://box/srv/other", "npm: dev.server");
        assert_ne!(a, b);
    }

    #[test]
    fn remote_pidfiles_are_unique() {
        assert_ne!(new_remote_pidfile(), new_remote_pidfile());
    }
}
