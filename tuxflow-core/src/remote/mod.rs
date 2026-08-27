pub mod fs;
pub mod git;
pub mod icon;
pub mod mic;
pub mod ports;
pub mod probe;
pub mod tunnel;
pub mod vite;

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

/// How many short-lived ssh commands may run at once.
///
/// Loading a workspace fires one probe per project with nothing pacing them.
/// Measured against a stock sshd, a burst of 29 simultaneous probes lost ~2 of
/// them; the same 29 throttled to 4 lost none across repeated runs. The host
/// refuses part of the burst (`MaxStartups` drops new connections past 10
/// in-flight, `MaxSessions` caps channels per connection at 10), and a refused
/// probe surfaces as "Can't reach <host>" for a host that is perfectly fine.
///
/// This bounds *probes* only. Long-lived process sessions are not throttled —
/// 14 concurrent ones were verified fine, since ControlMaster=auto opens a
/// fresh connection when it cannot multiplex.
const MAX_CONCURRENT_SSH: usize = 4;

static SSH_SLOTS: std::sync::LazyLock<(std::sync::Mutex<usize>, std::sync::Condvar)> =
    std::sync::LazyLock::new(|| (std::sync::Mutex::new(0), std::sync::Condvar::new()));

/// Held for the duration of one ssh command; releases the slot on drop.
pub struct SshPermit;

impl Drop for SshPermit {
    fn drop(&mut self) {
        let (lock, cvar) = &*SSH_SLOTS;
        if let Ok(mut n) = lock.lock() {
            *n = n.saturating_sub(1);
        }
        cvar.notify_one();
    }
}

/// Wait for a free ssh slot. **Blocking — worker threads only.** Blocking the
/// GTK main thread here would freeze the UI; the alternative (polling from
/// `idle_add_local`) busy-spins the main loop, which is why this parks a
/// thread instead.
pub fn ssh_permit() -> SshPermit {
    let (lock, cvar) = &*SSH_SLOTS;
    let mut n = lock.lock().unwrap_or_else(|e| e.into_inner());
    while *n >= MAX_CONCURRENT_SSH {
        n = cvar
            .wait(n)
            .unwrap_or_else(|e: std::sync::PoisonError<_>| e.into_inner());
    }
    *n += 1;
    SshPermit
}

/// Name of the dedicated tmux server socket (`tmux -L`). A separate socket
/// keeps TuxFlow's sessions and options away from the user's own tmux.
pub const TMUX_SOCKET: &str = "tuxflow";

/// Server options applied (idempotently) on every spawn, before any session
/// exists. `exit-empty off` is load-bearing: without it the server started by
/// the option-setting invocation exits immediately (no sessions yet) and
/// `new-session` would boot a fresh server that never saw these options.
/// The rest: no status bar so the pane looks like a plain terminal, mouse so
/// the wheel scrolls tmux history, deep history for long-running dev servers.
/// No default-shell needed — tmux always runs shell-commands via /bin/sh, so
/// the inner wrapper is POSIX-safe regardless of the user's login shell.
///
/// `window-size smallest` matters when the same project is open on two
/// machines: session names are deterministic, so both attach to the *same*
/// session, and tmux can only draw one window at one size. The default
/// (`latest`) sizes to whichever client moved last, which clips the other
/// client's view and pads the overhang with `·`. `smallest` fits the window
/// inside every attached client instead — nothing is ever hidden, at the cost
/// of unused margin on the larger screen. `aggressive-resize` re-fits as soon
/// as a client detaches rather than waiting for the next window switch.
/// (`\;` survives the remote shell as a literal `;`, chaining tmux commands.)
///
/// `set-clipboard on` is what makes an application's copy reachable at all.
/// Claude Code and friends copy by emitting OSC 52 and telling the user it
/// worked; VTE implements no OSC 52 (through 0.84), so the sequence would
/// die at the terminal. `on` makes tmux *accept* it into a paste buffer,
/// which is the one place the clipboard bridge can find it — with `off`
/// tmux discards it and the agent's "copied!" is a lie. The cost is that
/// tmux also re-emits OSC 52 outward to every attached client, so a session
/// attached from a terminal that honours it (kitty, foot on another
/// machine — session names are deterministic) sees copies made here. That
/// is tolerable; publishing someone's *old* buffer as a fresh selection is
/// not, and that is guarded by buffer age in `fetch_tmux_buffer` rather
/// than by throwing this option away.
const TMUX_OPTIONS: &str = "set -g exit-empty off \\; \
     set -g status off \\; set -g mouse on \\; set -g set-clipboard on \\; \
     set -g history-limit 50000 \\; set -g escape-time 10 \\; \
     set -g window-size smallest \\; setw -g aggressive-resize on \\; \
     set -g set-titles on \\; set -g set-titles-string '#{pane_title}'";

/// FNV-1a by hand — std's DefaultHasher isn't guaranteed stable across
/// releases, and these hashes end up in persistent names (tmux sessions,
/// icon cache files).
fn fnv64_iter(bytes: impl Iterator<Item = u8>) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

/// Stable hash of a string, e.g. a project key, for cache file names.
pub fn fnv64(s: &str) -> u64 {
    fnv64_iter(s.bytes())
}

/// Deterministic tmux session name for one process of one project. Stable
/// across app restarts so a fresh TuxFlow launch reattaches to sessions left
/// running by the previous one. FNV-1a by hand — std's DefaultHasher isn't
/// guaranteed stable across releases. tmux forbids `.`/`:` in names; the
/// slug keeps only shell-innocuous characters.
pub fn remote_session_name(project_key: &str, process_name: &str) -> String {
    let hash = fnv64_iter(project_key.bytes().chain([0u8]).chain(process_name.bytes()));
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
    env: &std::collections::BTreeMap<String, String>,
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
    // `$pf` lets the wrapper clean its own pidfile up on the way out — by
    // then the login-shell PID inside is dead anyway (kills of tmux-backed
    // processes go through the session name).
    let (pid_capture, pid_cleanup) = pidfile
        .map(|f| {
            (
                format!("pf={}; echo $$ > \"$pf\" && ", sh_quote(f)),
                "rm -f \"$pf\"; ".to_string(),
            )
        })
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
    // Exit-code + captured-output files, namespaced by remote uid at runtime:
    // session names are deterministic, so two users of the same host would
    // otherwise fight over one /tmp path — sticky /tmp makes the loser's
    // writes fail silently and exit codes vanish. `$ef`/`$of` are expanded by
    // the outer shell, so the inner command (built by string concatenation
    // below) bakes in the same paths.
    let ef_assign =
        format!("ef=\"/tmp/.{session}-$(id -u).exit\"; of=\"/tmp/.{session}-$(id -u).out\"");
    // Pane side: run the command, record its exit code for the wrapper, then
    // capture the pane's scrollback (with colors). tmux attaches on the
    // alternate screen, so everything shown during the run vanishes when the
    // client exits — the wrapper replays this capture onto the primary screen
    // afterwards so finished output stays visible in the terminal. tmux runs
    // shell-commands via /bin/sh, so this is POSIX-safe; inside the pane,
    // $TMUX makes a bare `tmux` target the right server and pane.
    let inner = format!(
        "{}\"$ef\"{}\"$of\"",
        sh_quote(&format!("{login_shell}; echo $? > ")),
        sh_quote("; tmux capture-pane -peJ -S -2000 > ")
    );
    let kill_stale = if fresh_session {
        format!("tmux -L {TMUX_SOCKET} kill-session -t {session_q} 2>/dev/null; ")
    } else {
        String::new()
    };
    // After the client exits: the exit-file means the command finished — that
    // code wins. No exit-file means the session is still alive (detach) or
    // the attach itself failed — pass the tmux client's status through so
    // real failures aren't masked as clean exits. A captured-output file is
    // replayed first (awk drops the blank tail rows the pane capture pads
    // with), putting the finished command's output back on screen. The
    // wrapper removes its pidfile and the read exit-file on the way out so
    // /tmp doesn't accumulate a file per spawn.
    //
    // The gsub strips OSC 52 out of that replay. `capture-pane -e` re-emits
    // the pane's escape sequences so colours survive, and replayed bytes are
    // parsed by the terminal exactly like live output — a clipboard-set
    // sequence in there would be *re-executed* at session exit, silently
    // republishing whatever some program copied during the run over whatever
    // the user has copied since. Nothing should put one in the capture today
    // (tmux consumes OSC 52 rather than storing it, and `off` above means it
    // no longer even accepts one), so this is the belt to that braces: a
    // replay must never be able to touch the clipboard, whatever ends up in
    // the scrollback. Both terminators are covered — BEL and ESC-backslash.
    let remote = format!(
        "cd {dir_q} && {pid_capture}\
         if command -v tmux >/dev/null 2>&1; then \
         {ef_assign}; rm -f \"$ef\" \"$of\"; {kill_stale}\
         tmux -L {TMUX_SOCKET} -f /dev/null start-server \\; {TMUX_OPTIONS}; \
         tmux -L {TMUX_SOCKET} new-session -A -s {session_q} -c {dir_q} {inner}; tst=$?; \
         if [ -f \"$of\" ]; then \
         awk '{{gsub(/\\033\\]52;[^\\007\\033]*(\\007|\\033\\\\)/,\"\")}} \
         NF{{n=NR}} {{l[NR]=$0}} END{{for(i=1;i<=n;i++) print l[i]}}' \"$of\"; \
         rm -f \"$of\"; fi; \
         {pid_cleanup}\
         if [ -f \"$ef\" ]; then s=\"$(cat \"$ef\")\"; rm -f \"$ef\"; exit \"$s\"; \
         else exit \"$tst\"; fi; \
         else exec {login_shell}; fi"
    );
    // LogLevel=ERROR mutes the mux client's "Shared connection … closed"
    // noise after the replayed output; real errors still print.
    format!(
        "exec ssh -t -o LogLevel=ERROR {} {} {}",
        ssh_mux_options_str(),
        sh_quote(host),
        sh_quote(&remote)
    )
}

/// The newest tmux paste buffer on a host, and how long ago tmux made it.
pub struct TmuxBuffer {
    pub text: String,
    /// Age at the moment the host answered. Measured entirely on the host,
    /// so a local clock that disagrees can't make an old buffer look new.
    pub age: std::time::Duration,
}

/// Newest tmux paste buffer on `host` — the mouse-selection clipboard
/// bridge. tmux stores mouse selections in its paste buffers, but no
/// released VTE implements OSC 52 (checked through 0.84), so the selection
/// can't reach the local clipboard through the terminal. TuxFlow fetches it
/// after a selection gesture instead.
///
/// The age comes back with it because "newest buffer" is *not* the same
/// question as "what did the user just select". tmux keeps a stack of them:
/// the newest may be the drag we're responding to, or it may be an OSC 52
/// an agent sent twenty minutes ago that nothing has displaced since.
/// Publishing the second as if it were the first is what made the clipboard
/// revert to text nobody had selected — so the caller gets the age and
/// decides. `#{buffer_created}` is a unix timestamp; the host's own `date`
/// comes back in the same round trip to subtract it from.
///
/// Blocking (one ssh round trip over the warm mux) — worker thread only.
pub fn fetch_tmux_buffer(host: &str) -> Option<TmuxBuffer> {
    let out = std::process::Command::new("ssh")
        .args(ssh_mux_options())
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"])
        .arg(host)
        .arg("--")
        // Newest first by creation time rather than by name: names are
        // recycled (buffer0 reappears once the stack drains), so sorting
        // them would eventually pick the wrong one.
        .arg(format!(
            "tmux -L {TMUX_SOCKET} list-buffers -F '#{{buffer_created}} #{{buffer_name}}' \
             2>/dev/null | sort -rn | head -1 | \
             {{ read -r c n || exit 0; echo \"$(date +%s) $c\"; \
             tmux -L {TMUX_SOCKET} show-buffer -b \"$n\" 2>/dev/null; }}"
        ))
        .output()
        .ok()?;
    parse_tmux_buffer(&String::from_utf8_lossy(&out.stdout))
}

/// Split the reply above into its `<now> <created>` header and the buffer
/// body. Separate from the ssh call so the parsing is testable.
fn parse_tmux_buffer(reply: &str) -> Option<TmuxBuffer> {
    let (header, text) = reply.split_once('\n')?;
    let (now, created) = header.split_once(' ')?;
    let now: i64 = now.trim().parse().ok()?;
    let created: i64 = created.trim().parse().ok()?;
    if text.is_empty() {
        return None;
    }
    Some(TmuxBuffer {
        text: text.to_string(),
        // A buffer created "in the future" means the host's clock moved
        // between the two reads; treat it as brand new rather than panic
        // on the negative.
        age: std::time::Duration::from_secs(now.saturating_sub(created).max(0) as u64),
    })
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
/// Shell snippet that installs the xclip shim if missing. Prefix for any
/// script that wants the agent's next Ctrl+V to read our clipboard file.
fn shim_provision_script() -> String {
    format!(
        "mkdir -p ~/.cache/tuxflow ~/.local/bin && \
         if [ ! -e ~/.local/bin/xclip ]; then \
         printf '%s' {} > ~/.local/bin/xclip && chmod +x ~/.local/bin/xclip; fi",
        sh_quote(CLIPBOARD_SHIM)
    )
}

pub fn upload_clipboard_image(host: &str, png: &[u8]) -> Result<String, String> {
    let script = format!(
        "{} && cat > ~/.cache/tuxflow/clipboard.png && \
         echo \"$HOME/.cache/tuxflow/clipboard.png\"",
        shim_provision_script()
    );
    ssh_stream_stdin(host, &script, png)
}

/// Stage an already-uploaded image file as the shim's clipboard content
/// (fresh mtime), so the agent's next Ctrl+V attaches it natively. Used by
/// the composer to deliver each pending attachment at send time. Blocking —
/// call from a worker thread.
pub fn stage_clipboard_image(host: &str, remote_path: &str) -> Result<(), String> {
    let script = format!(
        "{} && cp -f {} ~/.cache/tuxflow/clipboard.png",
        shim_provision_script(),
        sh_quote(remote_path)
    );
    ssh_stream_stdin(host, &script, &[]).map(|_| ())
}

/// Upload PNG bytes to a unique file under /tmp on `host` and return the
/// remote path. Used by the composer's attachment chips: each image needs
/// its own file (the clipboard shim above is a single rotating slot), and
/// /tmp guarantees nothing outlives a reboot. `stamp` disambiguates
/// concurrent uploads. Blocking — call from a worker thread.
pub fn upload_temp_image(host: &str, png: &[u8], stamp: u128) -> Result<String, String> {
    let path = format!("/tmp/.tuxflow-img-{stamp}.png");
    let script = format!("cat > {p} && echo {p}", p = sh_quote(&path));
    ssh_stream_stdin(host, &script, png)
}

/// Run `script` on `host` with `stdin_bytes` streamed to its stdin; returns
/// trimmed stdout on success, trimmed stderr on failure.
fn ssh_stream_stdin(host: &str, script: &str, stdin_bytes: &[u8]) -> Result<String, String> {
    use std::io::Write;
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
        .write_all(stdin_bytes)
        .map_err(|e| format!("failed to stream stdin: {e}"))?;
    let out = child
        .wait_with_output()
        .map_err(|e| format!("ssh failed: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// The remote half of an explicit stop, as a shell script. Split out from
/// [`remote_kill`] so the escalation can be pinned by tests without a host.
fn remote_kill_script(pidfile: &str, tmux_session: Option<&str>) -> String {
    let tmux_kill = tmux_session
        .map(|s| {
            let s = sh_quote(s);
            // Ask the foreground program to exit before killing the session
            // out from under it: a tool only gets to run its cleanup if it
            // sees the interrupt. Vite is the motivating case — on exit it
            // removes the `public/hot` file naming its dev server, and an
            // orphaned one leaves the app serving asset URLs that point at a
            // port nothing listens on (a page of refused `@vite` requests,
            // long after the run that wrote it). Poll rather than sleep a
            // flat grace, so a program that exits promptly is stopped
            // promptly; then kill whatever ignored the interrupt. Bounded at
            // ~2 s: agents (claude &c.) treat C-c as "interrupt", not "quit",
            // and are expected to reach the kill below.
            // Resolve the session's unique id ($N) up front and operate on
            // that, never the name: names are deterministic and reused, so a
            // restart respawns *this* name within the grace window below —
            // waiting on the name would then kill the fresh session we just
            // started. An id is never reissued.
            format!(
                "tid=$(tmux -L {TMUX_SOCKET} display-message -p -t {s} '#{{session_id}}' 2>/dev/null); \
                 if [ -n \"$tid\" ]; then \
                   tmux -L {TMUX_SOCKET} send-keys -t \"$tid\" C-c 2>/dev/null; \
                   i=0; while [ $i -lt 10 ]; do \
                     tmux -L {TMUX_SOCKET} has-session -t \"$tid\" 2>/dev/null || break; \
                     sleep 0.2; i=$((i+1)); \
                   done; \
                   tmux -L {TMUX_SOCKET} kill-session -t \"$tid\" 2>/dev/null; \
                 fi; "
            )
        })
        .unwrap_or_default();
    format!(
        "{tmux_kill}sid=$(cat {p} 2>/dev/null); rm -f {p}; \
         if [ -n \"$sid\" ]; then \
           pkill -TERM -s \"$sid\" 2>/dev/null; sleep 1; \
           pkill -KILL -s \"$sid\" 2>/dev/null; \
         fi; true",
        p = sh_quote(pidfile)
    )
}

/// Explicitly kill the remote side of a process: the tmux session (when one
/// was used, interrupted first so it can clean up after itself) and the login
/// session recorded in `pidfile` (TERM, then KILL after a grace second —
/// covers the no-tmux fallback and SIGHUP-trappers).
/// Runs fire-and-forget on a worker thread — never blocks the GTK main
/// thread. (On the fallback path, processes that daemonize into a *new*
/// session still escape; with tmux the session kill catches them.)
pub fn remote_kill(host: &str, pidfile: &str, tmux_session: Option<&str>) {
    let script = remote_kill_script(pidfile, tmux_session);
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

    /// The cap is the whole point: without it a workspace load opened one ssh
    /// channel per project and sshd refused everything past MaxSessions.
    #[test]
    fn ssh_permits_never_exceed_the_cap() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let live = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();

        for _ in 0..29 {
            let live = live.clone();
            let peak = peak.clone();
            handles.push(std::thread::spawn(move || {
                let _permit = super::ssh_permit();
                let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(5));
                live.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for h in handles {
            h.join().expect("worker thread panicked");
        }

        assert_eq!(live.load(Ordering::SeqCst), 0, "permits leaked");
        let peak = peak.load(Ordering::SeqCst);
        assert!(
            peak <= super::MAX_CONCURRENT_SSH,
            "{peak} concurrent ssh commands exceeded the cap of {}",
            super::MAX_CONCURRENT_SSH
        );
        assert!(peak > 1, "throttle serialised everything (peak {peak})");
    }
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
        let env = std::collections::BTreeMap::from([("PORT".to_string(), "3000".to_string())]);
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
    fn remote_kill_interrupts_before_killing_the_session() {
        let script = remote_kill_script("/tmp/.tuxflow-abc-0.pid", Some("tf-dev-1"));
        let interrupt = script.find("send-keys").expect("sends an interrupt");
        let wait = script.find("has-session").expect("waits for a clean exit");
        let kill = script.find("kill-session").expect("still force-kills");
        // Order is the whole point: a program that cleans up on C-c only gets
        // to if the interrupt lands before the session is torn down.
        assert!(
            interrupt < wait && wait < kill,
            "escalation out of order: {script}"
        );
        // The pidfile sweep still runs after the session is gone.
        assert!(script.find("pkill -TERM").expect("sweeps pidfile") > kill);
    }

    #[test]
    fn remote_kill_grace_targets_session_id_not_name() {
        // Session names are deterministic and reused, so a restart recreates
        // this very name inside the grace window — every operation after the
        // initial lookup must target the resolved id, or the wait ends by
        // killing the freshly spawned session instead of the old one.
        let script = remote_kill_script("/tmp/p", Some("tf-dev-1"));
        let lookup = script.find("display-message").expect("resolves session id");
        assert!(script.contains("'#{session_id}'"));
        for op in ["send-keys", "has-session", "kill-session"] {
            let at = script.find(op).unwrap_or_else(|| panic!("{op} missing"));
            assert!(at > lookup, "{op} runs before the id lookup");
            let arg = &script[at..];
            let arg = &arg[..arg.find(';').unwrap_or(arg.len())];
            assert!(arg.contains("\"$tid\""), "{op} targets a name, not the id");
        }
    }

    #[test]
    fn remote_kill_without_tmux_has_nothing_to_interrupt() {
        // The no-tmux fallback exec's the command directly — there is no
        // session to send keys to, so it goes straight to the pidfile sweep.
        let script = remote_kill_script("/tmp/.tuxflow-abc-0.pid", None);
        assert!(!script.contains("send-keys"));
        assert!(!script.contains("kill-session"));
        assert!(script.contains("pkill -TERM"));
        assert!(script.contains("pkill -KILL"));
    }

    #[test]
    fn remote_kill_quotes_a_hostile_session_name() {
        let script = remote_kill_script("/tmp/p'f", Some("tf-dev-1; rm -rf /"));
        assert!(!script.contains("; rm -rf /;"));
        assert!(script.contains(r"'tf-dev-1; rm -rf /'"));
    }

    #[test]
    fn wrap_remote_command_fresh_kills_stale_session() {
        let env = std::collections::BTreeMap::new();
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
        let env = std::collections::BTreeMap::new();
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

    /// An agent copies by emitting OSC 52 and reporting success. VTE has no
    /// OSC 52, so `set-clipboard on` — tmux accepting it into a paste buffer
    /// — is the only thing standing between that and the copy vanishing.
    /// Mouse mode matters for the same reason: it is what puts a drag's
    /// selection into a buffer for the bridge to find.
    #[test]
    fn tmux_bootstrap_keeps_copies_reachable() {
        let env = std::collections::BTreeMap::new();
        let cmd = wrap_remote_command("box", "/srv/app", &env, "npm run dev", None, "tf-a", false);
        assert!(cmd.contains("set-clipboard on"));
        assert!(cmd.contains("set -g mouse on"));
    }

    /// The reply carries the host's clock and the buffer's creation time so
    /// the caller can tell a selection just made from a buffer some program
    /// left lying around — publishing the latter as the former is what made
    /// the clipboard revert to text nobody selected.
    #[test]
    fn buffer_age_comes_from_the_hosts_own_clock() {
        let fresh = parse_tmux_buffer("1787016638 1787016638\njust selected this").expect("parses");
        assert_eq!(fresh.text, "just selected this");
        assert_eq!(fresh.age.as_secs(), 0);

        let stale =
            parse_tmux_buffer("1787016638 1787015000\nan agent copied this").expect("parses");
        assert_eq!(stale.age.as_secs(), 1638);
    }

    #[test]
    fn buffer_text_keeps_its_own_newlines() {
        let buf = parse_tmux_buffer("100 90\nfirst line\nsecond line\n").expect("parses");
        assert_eq!(buf.text, "first line\nsecond line\n");
        assert_eq!(buf.age.as_secs(), 10);
    }

    /// No buffers on the server: the reply is empty and there is nothing to
    /// publish — not an empty string to overwrite the clipboard with.
    #[test]
    fn an_empty_buffer_list_yields_nothing() {
        assert!(parse_tmux_buffer("").is_none());
        assert!(
            parse_tmux_buffer("100 90\n").is_none(),
            "header but no text"
        );
        assert!(parse_tmux_buffer("garbage\ntext").is_none());
    }

    /// A host whose clock stepped between the two reads would otherwise
    /// produce a negative age, and an age that underflows into "ancient"
    /// silently disables the selection bridge.
    #[test]
    fn a_backwards_clock_reads_as_brand_new() {
        let buf = parse_tmux_buffer("100 140\nselected").expect("parses");
        assert_eq!(buf.age.as_secs(), 0);
    }

    /// Replayed scrollback is parsed exactly like live output, so an OSC 52
    /// surviving in the capture would be *re-executed* when a session exits,
    /// overwriting whatever the user copied since. This runs the wrapper's
    /// real awk program — quoting and all, as the remote shell would see it
    /// — over a capture fixture, so a regression in those escape levels
    /// fails here rather than silently letting the sequence through.
    #[test]
    fn replayed_capture_cannot_set_the_clipboard() {
        let env = std::collections::BTreeMap::new();
        let cmd = wrap_remote_command("box", "/srv/app", &env, "npm run dev", None, "tf-a", false);
        // Unwrap one level of sh_quote: the whole remote script is single
        // quoted for ssh, so its own quotes arrive as '\''.
        let start = cmd.find(r"awk '\''").expect("replay runs through awk") + r"awk '\''".len();
        let prog = &cmd[start..];
        let prog = &prog[..prog.find(r#"'\'' "$of""#).expect("awk program ends")];
        assert!(!prog.contains('\''), "program can't be re-quoted safely");

        let fixture = std::env::temp_dir().join(format!("tuxflow-capture-{}", std::process::id()));
        std::fs::write(
            &fixture,
            "plain \x1b[31mred\x1b[0m line\n\
             \x1b]52;c;aGVsbG8=\x07after-bel\n\
             \x1b]52;c;d29ybGQ=\x1b\\after-st\n\n\n",
        )
        .expect("write capture fixture");
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!(
                "awk '{prog}' {}",
                sh_quote(&fixture.to_string_lossy())
            ))
            .output()
            .expect("run the replay filter");
        let _ = std::fs::remove_file(&fixture);
        let replayed = String::from_utf8_lossy(&out.stdout);

        assert!(!replayed.contains("]52;"), "OSC 52 survived: {replayed:?}");
        // Everything else is untouched: colours still render, the text that
        // followed the stripped sequence stays, blank tail rows still go.
        assert!(replayed.contains("\x1b[31mred\x1b[0m"));
        assert!(replayed.contains("after-bel") && replayed.contains("after-st"));
        assert!(
            replayed.ends_with("after-st\n"),
            "tail rows kept: {replayed:?}"
        );
    }
}
