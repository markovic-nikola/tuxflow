//! End-to-end tests for the remote (ssh) plumbing, hermetic via a fake `ssh`
//! binary placed first in PATH. The fake executes the "remote" command in a
//! local shell, so everything except the network layer — argument
//! construction, sh quoting, cd/env wrapping, FS probe batching — runs for
//! real.

use std::fs;
use std::path::Path;

use tempfile::TempDir;

use tuxflow_core::detect::detector::detect_stacks_fs;
use tuxflow_core::remote::fs::{ProjectFs, SshFs, remote_dir_exists};
use tuxflow_core::remote::{list_live_sessions, sh_quote, wrap_remote_command};

/// Install a fake `ssh` (once per test process) that runs the remote command
/// locally. Skips option flags the way OpenSSH would, so the exact
/// invocations TuxFlow builds are parsed for real.
fn install_fake_ssh() {
    static INSTALLED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    INSTALLED.get_or_init(|| {
        let bin_dir = TempDir::new().unwrap();
        let fake_ssh = bin_dir.path().join("ssh");
        fs::write(
            &fake_ssh,
            r#"#!/bin/bash
# Fake ssh: consume openssh-style args, run the command in a local shell.
args=("$@")
host=""
cmd=""
i=0
while [ $i -lt ${#args[@]} ]; do
  a="${args[$i]}"
  case "$a" in
    -o|-L|-i|-p) i=$((i+2)); continue ;;
    -t|-N|--) i=$((i+1)); continue ;;
    -*) i=$((i+1)); continue ;;
    *)
      if [ -z "$host" ]; then
        host="$a"
      else
        if [ -n "$cmd" ]; then cmd="$cmd $a"; else cmd="$a"; fi
      fi
      i=$((i+1))
      ;;
  esac
done
if [ -z "$cmd" ]; then
  # ssh -N (tunnel): just idle
  exec sleep 30
fi
exec sh -c "$cmd"
"#,
        )
        .unwrap();
        let mut perms = fs::metadata(&fake_ssh).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o755);
        fs::set_permissions(&fake_ssh, perms).unwrap();

        let path = std::env::var("PATH").unwrap_or_default();
        unsafe {
            std::env::set_var(
                "PATH",
                format!("{}:{path}", bin_dir.path().to_string_lossy()),
            );
        }
        // Keep the dir alive for the whole test process
        std::mem::forget(bin_dir);
    });
}

fn has_tmux() -> bool {
    std::process::Command::new("tmux")
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Run a wrapped remote command the way ProcessManager does, but under a PTY
/// (via util-linux `script`): when tmux is present the wrap attaches to a
/// session, which requires a terminal. `-e` propagates the child's exit code.
/// `TMUX_TMPDIR=dir` gives each test its own tmux server, isolated from the
/// user's and from other tests; clean it up with `kill_test_tmux(dir)`.
fn run_wrapped(wrapped: &str, dir: &Path) -> std::process::ExitStatus {
    let sh = dir.join(format!(
        "wrapped-{}.sh",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::write(&sh, format!("#!/bin/bash\n{wrapped}\n")).unwrap();
    std::process::Command::new("script")
        .args(["-qec", &format!("bash {}", sh.display()), "/dev/null"])
        // tmux refuses to attach without a capable TERM; VTE provides one
        // in production, the bare test env may not.
        .env("TERM", "xterm")
        .env("TMUX_TMPDIR", dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
}

/// Like `run_wrapped`, but records the terminal transcript (what a user
/// would see in the VTE) and returns it alongside the exit status.
fn run_wrapped_capture(wrapped: &str, dir: &Path) -> (std::process::ExitStatus, String) {
    let sh = dir.join(format!(
        "wrapped-cap-{}.sh",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::write(&sh, format!("#!/bin/bash\n{wrapped}\n")).unwrap();
    let typescript = dir.join("typescript.out");
    let status = std::process::Command::new("script")
        .args([
            "-qec",
            &format!("bash {}", sh.display()),
            &typescript.to_string_lossy(),
        ])
        .env("TERM", "xterm")
        .env("TMUX_TMPDIR", dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    let transcript = fs::read(&typescript)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    (status, transcript)
}

/// Kill the per-test tmux server, if any — the production wrap keeps it
/// alive between spawns (`exit-empty off`), which in tests would leak one
/// server process per run.
fn kill_test_tmux(dir: &Path) {
    let _ = std::process::Command::new("tmux")
        .args(["-L", "tuxflow", "kill-server"])
        .env("TMUX_TMPDIR", dir)
        .output();
}

fn seed_project(dir: &Path) {
    fs::write(
        dir.join("package.json"),
        r#"{"scripts":{"dev":"vite","build":"vite build"}}"#,
    )
    .unwrap();
    fs::write(dir.join("pnpm-lock.yaml"), "").unwrap();
    fs::write(
        dir.join("tuxflow.toml"),
        "[project]\nname = \"remote-app\"\n",
    )
    .unwrap();
}

#[test]
fn ssh_fs_end_to_end_via_fake_ssh() {
    install_fake_ssh();
    let project = TempDir::new().unwrap();
    seed_project(project.path());
    let root = project.path().to_string_lossy().to_string();

    let sshfs = SshFs::new("testhost", &root);

    // remote_dir_exists distinguishes present/absent
    assert_eq!(remote_dir_exists("testhost", &root), Ok(true));
    assert_eq!(
        remote_dir_exists("testhost", &format!("{root}/nope")),
        Ok(false)
    );

    // read_to_string fetches file content
    let toml = sshfs.read_to_string("tuxflow.toml").unwrap();
    assert!(toml.contains("remote-app"));
    assert!(sshfs.read_to_string("missing.file").is_err());

    // exists / exists_many (single round trip) agree with reality
    assert!(sshfs.exists("package.json"));
    assert!(!sshfs.exists("Cargo.toml"));
    assert_eq!(
        sshfs.exists_many(&["package.json", "Cargo.toml", "pnpm-lock.yaml"]),
        vec![true, false, true]
    );

    // Full stack detection over the ssh transport, including the
    // package-manager lockfile probe (pnpm here).
    let stacks = detect_stacks_fs(&sshfs);
    assert_eq!(stacks.len(), 1);
    assert_eq!(stacks[0].name, "Node.js");
    let dev = stacks[0]
        .suggested_processes
        .iter()
        .find(|p| p.name == "dev")
        .unwrap();
    assert_eq!(dev.command, "pnpm dev");
}

#[test]
fn wrapped_remote_command_runs_in_cwd_with_env() {
    install_fake_ssh();
    let project = TempDir::new().unwrap();
    let root = project.path().to_string_lossy().to_string();
    let marker = project.path().join("it's a marker.txt"); // exercise quoting

    let env = std::collections::BTreeMap::from([
        ("TUX_TEST_VAL".to_string(), "hello world".to_string()),
        ("OTHER".to_string(), "with'quote".to_string()),
    ]);
    // The command TuxFlow would put into the VTE argv slot
    let inner = format!(
        "printf '%s|%s' \"$TUX_TEST_VAL\" \"$OTHER\" > {}",
        sh_quote(&marker.to_string_lossy())
    );
    let pidfile = project.path().join("session.pid");
    let session = format!("tf-test-env-{}", std::process::id());
    let wrapped = wrap_remote_command(
        "testhost",
        &root,
        &env,
        &inner,
        Some(&pidfile.to_string_lossy()),
        &session,
        false,
    );

    let status = run_wrapped(&wrapped, project.path());
    assert!(status.success(), "wrapped command failed: {wrapped}");

    let content = fs::read_to_string(&marker).unwrap();
    assert_eq!(content, "hello world|with'quote");

    // Pidfile lifecycle differs by path: the tmux wrapper deletes it on the
    // way out (/tmp hygiene — the PID inside is dead by then), while the
    // no-tmux fallback `exec`s the command and can't clean up after itself.
    if has_tmux() {
        assert!(
            !pidfile.exists(),
            "tmux wrapper should remove its pidfile on exit"
        );
    } else {
        let pid: i32 = fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .expect("pidfile should contain a PID");
        assert!(pid > 0);
    }
    kill_test_tmux(project.path());
}

#[test]
fn wrapped_remote_command_propagates_exit_code() {
    install_fake_ssh();
    let project = TempDir::new().unwrap();
    let root = project.path().to_string_lossy().to_string();
    let session = format!("tf-test-exit-{}", std::process::id());
    let env = std::collections::BTreeMap::new();

    // With tmux the code travels via the session exit-file; without it the
    // fallback execs directly — either way the ssh client must exit 7.
    let wrapped = wrap_remote_command("testhost", &root, &env, "exit 7", None, &session, false);
    let status = run_wrapped(&wrapped, project.path());
    assert_eq!(status.code(), Some(7), "wrapped: {wrapped}");
    kill_test_tmux(project.path());
}

#[test]
fn wrapped_remote_command_replays_output_after_exit() {
    install_fake_ssh();
    if !has_tmux() {
        eprintln!("skipping: tmux not installed");
        return;
    }
    let project = TempDir::new().unwrap();
    let root = project.path().to_string_lossy().to_string();
    let session = format!("tf-test-replay-{}", std::process::id());
    let env = std::collections::BTreeMap::new();

    // tmux attaches on the alternate screen, so the live output vanishes
    // when the session ends — the wrap must replay the captured pane onto
    // the primary screen. Transcript then holds the marker at least twice:
    // once live (alt screen) and once replayed.
    let wrapped = wrap_remote_command(
        "testhost",
        &root,
        &env,
        "echo REPLAY_MARKER_XYZ",
        None,
        &session,
        false,
    );
    let (status, transcript) = run_wrapped_capture(&wrapped, project.path());
    assert!(status.success(), "wrapped: {wrapped}");
    let count = transcript.matches("REPLAY_MARKER_XYZ").count();
    assert!(
        count >= 2,
        "expected live + replayed output, marker seen {count}x"
    );
    kill_test_tmux(project.path());
}

#[test]
fn tmux_session_survives_client_death_and_reattaches() {
    install_fake_ssh();
    if !has_tmux() {
        eprintln!("skipping: tmux not installed");
        return;
    }
    let project = TempDir::new().unwrap();
    let root = project.path().to_string_lossy().to_string();
    let session = format!("tf-test-persist-{}", std::process::id());
    let env = std::collections::BTreeMap::new();
    // list_live_sessions runs tmux through the fake ssh with the *process*
    // env — point it at this test's isolated server. Concurrent tests set
    // TMUX_TMPDIR per-command, so this global doesn't affect them.
    unsafe {
        std::env::set_var("TMUX_TMPDIR", project.path());
    }
    // Each (re)run of the command appends a line — a reattach must not add one
    let cmd = "echo run >> runs.log; sleep 30";
    let wrapped = wrap_remote_command("testhost", &root, &env, cmd, None, &session, false);

    let sh = project.path().join("client.sh");
    fs::write(&sh, format!("#!/bin/bash\n{wrapped}\n")).unwrap();
    let spawn_client = || {
        std::process::Command::new("script")
            .args(["-qec", &format!("bash {}", sh.display()), "/dev/null"])
            .env("TERM", "xterm")
            .env("TMUX_TMPDIR", project.path())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap()
    };
    let tmux = |args: &[&str]| {
        std::process::Command::new("tmux")
            .args(["-L", "tuxflow"])
            .args(args)
            .env("TMUX_TMPDIR", project.path())
            .output()
            .unwrap()
    };
    let wait_for = |pred: &dyn Fn() -> bool| {
        for _ in 0..100 {
            if pred() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        false
    };

    let runs_log = project.path().join("runs.log");
    let mut client = spawn_client();
    assert!(
        wait_for(&|| runs_log.exists()),
        "command never started under tmux"
    );

    // Simulate a connection drop: kill the attached client. The pty
    // teardown HUPs the tmux client; the session must keep running.
    client.kill().unwrap();
    let _ = client.wait();
    assert!(
        wait_for(&|| tmux(&["has-session", "-t", &session]).status.success()),
        "session died with its client"
    );

    // The app-restart reattach path discovers this session over ssh
    let live = list_live_sessions("testhost");
    assert!(live.contains(&session), "live sessions: {live:?}");

    // Reattach (fresh = false). The command must NOT have re-run.
    let mut client2 = spawn_client();
    std::thread::sleep(std::time::Duration::from_secs(1));
    let runs = fs::read_to_string(&runs_log).unwrap();
    assert_eq!(runs.lines().count(), 1, "reattach restarted the command");

    // Cleanup: kill the whole per-test server; the attached client exits.
    kill_test_tmux(project.path());
    let _ = client2.wait();
}

/// `list_local_dirs` is the local twin of `list_remote_dirs`, and the
/// add-project completion widget drives both through one code path — so its
/// contract has to match `ls -1dp -- <prefix>*` where it is observable:
/// directories only, trailing slash, sorted, glob's dotfile rule.
mod list_local_dirs {
    use tempfile::TempDir;
    use tuxflow_core::remote::fs::list_local_dirs;

    fn fixture() -> TempDir {
        let tmp = TempDir::new().unwrap();
        for dir in ["alpha", "alpine", "beta", ".hidden"] {
            std::fs::create_dir(tmp.path().join(dir)).unwrap();
        }
        // A regular file must never be offered as a directory to descend.
        std::fs::write(tmp.path().join("alpha.txt"), b"x").unwrap();
        tmp
    }

    /// Trailing slash lists the directory's children.
    #[test]
    fn lists_children_of_a_complete_dir() {
        let tmp = fixture();
        let got = list_local_dirs(&format!("{}/", tmp.path().display()));
        let names: Vec<&str> = got.iter().map(|p| p.as_str()).collect();
        assert_eq!(names.len(), 3, "expected 3 visible dirs, got {names:?}");
        assert!(names.iter().all(|p| p.ends_with('/')), "{names:?}");
        assert!(names[0].ends_with("/alpha/"), "not sorted: {names:?}");
        assert!(
            !names.iter().any(|p| p.contains("alpha.txt")),
            "listed a plain file: {names:?}"
        );
    }

    /// A partial name filters mid-name, as the shell glob would.
    #[test]
    fn filters_on_a_partial_name() {
        let tmp = fixture();
        let got = list_local_dirs(&format!("{}/alp", tmp.path().display()));
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(got[0].ends_with("/alpha/"), "{got:?}");
        assert!(got[1].ends_with("/alpine/"), "{got:?}");
    }

    /// Glob semantics: a bare `*` skips dotfiles, but typing the dot reveals
    /// them. Without this the local half would offer `.git`/`.cache` on every
    /// directory while the remote half (real `ls`) would not.
    #[test]
    fn hidden_dirs_need_an_explicit_dot() {
        let tmp = fixture();
        let bare = list_local_dirs(&format!("{}/", tmp.path().display()));
        assert!(
            !bare.iter().any(|p| p.contains(".hidden")),
            "dotfile leaked into a bare listing: {bare:?}"
        );
        let dotted = list_local_dirs(&format!("{}/.", tmp.path().display()));
        assert_eq!(dotted.len(), 1, "{dotted:?}");
        assert!(dotted[0].ends_with("/.hidden/"), "{dotted:?}");
    }

    /// Relative input and unreadable parents yield nothing rather than
    /// guessing — the field only ever completes absolute paths.
    #[test]
    fn refuses_relative_and_missing() {
        assert!(list_local_dirs("alpha").is_empty());
        assert!(list_local_dirs("/nonexistent-zz/xx").is_empty());
    }
}
