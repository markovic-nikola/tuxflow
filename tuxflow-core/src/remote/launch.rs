//! Launch lists: each remote project's processes, written onto the host so
//! a client running THERE can start one exactly as TuxFlow would.
//!
//! The client is the `tf` picker a phone lands in over ssh. It can attach to
//! any live `tf-*` session on its own, but starting a stopped process needs
//! what only TuxFlow knows: the session name (a hash of the project key and
//! process name), the environment (MCP socket, agent hooks), the tmux
//! bootstrap and the exit-file protocol the desktop reads a run's end from.
//! Rather than re-derive any of that in shell, TuxFlow writes out the very
//! script its own spawn runs on the host ([`super::remote_launch_script`]),
//! so a phone-started agent is indistinguishable from a desktop-started one:
//! the desktop reattaches it on its next load, or when ▶ is pressed
//! (`new-session -A` attaches instead of starting a second copy).
//!
//! On the host, under `~/.local/state/tuxflow/launch/<project hash>/`:
//!
//! ```text
//! project              # tuxflow-launch 1 / # project: <name> / # dir: <dir>
//! 00-<session>.sh      # tuxflow-launch 1 / # process: … / # title: …
//! 01-<session>.sh      #   / # category: … / # session: …, then the script
//! ```
//!
//! Files are ordered as the sidebar orders them. The whole directory is
//! rebuilt aside and swapped in, so a reader never sees half a list. The
//! header lines are the format — `tf` parses them; bump [`FORMAT`] if they
//! change.

use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use super::{fnv64_iter, sh_quote};

/// Where the lists live, relative to the host user's home.
pub const LAUNCH_ROOT: &str = ".local/state/tuxflow/launch";

/// First header line of every file; a reader checks it before trusting the rest.
pub const FORMAT: &str = "tuxflow-launch 1";

/// Heredoc terminator in the publish script. An entry whose content
/// contains it as a whole line is skipped rather than allowed to cut its
/// file short and run the remainder as shell.
const EOF_MARK: &str = "TUXFLOW_LAUNCH_EOF";

/// How long one publish may take before it is abandoned (the next change
/// republishes anyway).
const PUBLISH_TIMEOUT: Duration = Duration::from_secs(20);

/// One startable process, as a client on the host sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchEntry {
    /// The process's name (stable identity, part of the session name).
    pub name: String,
    /// What the sidebar shows (display name, else the name).
    pub title: String,
    /// `agent`, `command` or `terminal`.
    pub category: String,
    /// The tmux session the script creates or attaches to.
    pub session: String,
    /// [`super::remote_launch_script`]'s output — run it with the host
    /// user's shell, in a terminal.
    pub script: String,
}

/// The project's directory under [`LAUNCH_ROOT`]: a hash of its key, since
/// two projects may share a basename.
pub fn project_dir_name(project_key: &str) -> String {
    format!("{:016x}", fnv64_iter(project_key.bytes()))
}

/// A header value on one line, whatever a name contains.
fn one_line(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

/// The shell script that replaces a project's list on the host. Pure, so
/// the format is testable without a host.
pub fn publish_script(
    project_key: &str,
    project_name: &str,
    project_dir: &str,
    entries: &[LaunchEntry],
) -> String {
    let dir = format!("\"$HOME\"/{LAUNCH_ROOT}/{}", project_dir_name(project_key));
    // umask 077: a process's env can carry tokens, and on a shared host
    // the files must be readable by their owner alone.
    let mut out =
        format!("set -e\numask 077\nd={dir}\nt=\"$d.tmp.$$\"\nrm -rf \"$t\"\nmkdir -p \"$t\"\n");
    let mut file = |name: &str, body: &str| {
        out.push_str(&format!(
            "cat > \"$t\"/{} <<'{EOF_MARK}'\n{body}\n{EOF_MARK}\n",
            sh_quote(name)
        ));
    };
    file(
        "project",
        &format!(
            "# {FORMAT}\n# project: {}\n# dir: {}\n# key: {}",
            one_line(project_name),
            one_line(project_dir),
            one_line(project_key)
        ),
    );
    for (i, entry) in entries.iter().enumerate() {
        let body = format!(
            "#!/bin/sh\n# {FORMAT}\n# process: {}\n# title: {}\n# category: {}\n# session: {}\n{}",
            one_line(&entry.name),
            one_line(&entry.title),
            one_line(&entry.category),
            one_line(&entry.session),
            entry.script
        );
        if body.lines().any(|l| l == EOF_MARK) {
            continue;
        }
        // Session names are tmux-safe slugs (`remote_session_name`), so
        // they are safe as file names too.
        file(&format!("{i:02}-{}.sh", entry.session), &body);
    }
    out.push_str("rm -rf \"$d\"\nmv \"$t\" \"$d\"\n");
    out
}

/// The shell script that removes a project's list (the project was removed
/// from TuxFlow).
pub fn unpublish_script(project_key: &str) -> String {
    format!(
        "rm -rf \"$HOME\"/{LAUNCH_ROOT}/{}",
        project_dir_name(project_key)
    )
}

struct Job {
    host: String,
    project_key: String,
    script: String,
}

static QUEUE: LazyLock<Mutex<Option<Sender<Job>>>> = LazyLock::new(|| Mutex::new(None));

/// Queue `script` (from [`publish_script`] / [`unpublish_script`]) to run on
/// `host`. Returns at once. One worker thread runs the queue in order, and a
/// burst of changes to the same project collapses to its latest script — a
/// slower, older publish can never land after a newer one.
pub fn submit(host: &str, project_key: &str, script: String) {
    let job = Job {
        host: host.to_string(),
        project_key: project_key.to_string(),
        script,
    };
    let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    // A send fails only if the worker died; start a fresh one with the job.
    let job = match queue.as_ref() {
        Some(tx) => match tx.send(job) {
            Ok(()) => return,
            Err(err) => err.0,
        },
        None => job,
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = tx.send(job);
    *queue = Some(tx);
    std::thread::spawn(move || worker(rx));
}

fn worker(rx: Receiver<Job>) {
    while let Ok(first) = rx.recv() {
        // Drain what queued up meanwhile; the latest script per project wins.
        let mut latest: BTreeMap<(String, String), String> = BTreeMap::new();
        latest.insert((first.host, first.project_key), first.script);
        while let Ok(job) = rx.try_recv() {
            latest.insert((job.host, job.project_key), job.script);
        }
        for ((host, key), script) in latest {
            if let Err(err) = run(&host, &script) {
                eprintln!("tuxflow: launch list for {key} not written: {err}");
            }
        }
    }
}

fn run(host: &str, script: &str) -> Result<(), String> {
    let _permit = super::ssh_permit();
    let mut cmd = std::process::Command::new("ssh");
    cmd.args(super::ssh_mux_options())
        .args(["-o", "BatchMode=yes", "-o", "LogLevel=ERROR"])
        .arg(host)
        .arg(script);
    let out = super::git::output_within(cmd, PUBLISH_TIMEOUT).map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, session: &str, script: &str) -> LaunchEntry {
        LaunchEntry {
            name: name.into(),
            title: name.into(),
            category: "agent".into(),
            session: session.into(),
            script: script.into(),
        }
    }

    /// Runs a script the way the host does — through a shell, with `$HOME`
    /// pointing at a scratch directory.
    fn run_local(home: &std::path::Path, script: &str) {
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(script)
            .env("HOME", home)
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn project_dirs_are_stable_and_distinct() {
        let a = project_dir_name("ssh://box/srv/app");
        assert_eq!(a, project_dir_name("ssh://box/srv/app"));
        assert_eq!(a.len(), 16);
        assert_ne!(a, project_dir_name("ssh://box/srv/other"));
    }

    #[test]
    fn publish_writes_one_file_per_process_in_order_and_replaces_the_old_list() {
        let home = tempfile::tempdir().unwrap();
        let key = "ssh://box/srv/app";
        let dir = home.path().join(LAUNCH_ROOT).join(project_dir_name(key));
        // A stale list from before: the publish must not leave it behind.
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("00-tf-gone-00000000.sh"), "old").unwrap();

        let script = "cd '/srv/app' && echo \"$HOME\" 'it''s' $ef";
        let entries = [
            entry("claude", "tf-claude-1234abcd", script),
            entry("dev\nserver", "tf-dev-server-5678ef01", "exit 0"),
        ];
        run_local(
            home.path(),
            &publish_script(key, "app", "/srv/app", &entries),
        );

        let mut files: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        files.sort();
        assert_eq!(
            files,
            [
                "00-tf-claude-1234abcd.sh",
                "01-tf-dev-server-5678ef01.sh",
                "project"
            ]
        );
        let first = std::fs::read_to_string(dir.join("00-tf-claude-1234abcd.sh")).unwrap();
        assert_eq!(
            first,
            format!(
                "#!/bin/sh\n# {FORMAT}\n# process: claude\n# title: claude\n# category: agent\n# session: tf-claude-1234abcd\n{script}\n"
            ),
            "the script arrives byte for byte, nothing expanded"
        );
        let second = std::fs::read_to_string(dir.join("01-tf-dev-server-5678ef01.sh")).unwrap();
        assert!(
            second.contains("# process: dev server\n"),
            "headers stay one line"
        );
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700, "env may hold tokens: owner only");
        assert_eq!(mode(&dir.join("00-tf-claude-1234abcd.sh")), 0o600);
        let project = std::fs::read_to_string(dir.join("project")).unwrap();
        assert!(project.starts_with(&format!("# {FORMAT}\n# project: app\n# dir: /srv/app\n")));
        assert!(
            !home
                .path()
                .join(LAUNCH_ROOT)
                .read_dir()
                .unwrap()
                .any(|e| { e.unwrap().file_name().to_string_lossy().contains(".tmp.") }),
            "no staging directory left behind"
        );
    }

    #[test]
    fn an_entry_holding_the_terminator_is_skipped_not_spliced() {
        let home = tempfile::tempdir().unwrap();
        let key = "ssh://box/srv/app";
        let entries = [
            entry(
                "evil",
                "tf-evil-00000001",
                &format!("echo\n{EOF_MARK}\ntouch \"$HOME/pwned\""),
            ),
            entry("ok", "tf-ok-00000002", "exit 0"),
        ];
        run_local(
            home.path(),
            &publish_script(key, "app", "/srv/app", &entries),
        );
        let dir = home.path().join(LAUNCH_ROOT).join(project_dir_name(key));
        assert!(!home.path().join("pwned").exists());
        assert!(!dir.join("00-tf-evil-00000001.sh").exists());
        assert!(dir.join("01-tf-ok-00000002.sh").exists());
    }

    #[test]
    fn unpublish_removes_only_that_project() {
        let home = tempfile::tempdir().unwrap();
        let (a, b) = ("ssh://box/srv/a", "ssh://box/srv/b");
        run_local(home.path(), &publish_script(a, "a", "/srv/a", &[]));
        run_local(home.path(), &publish_script(b, "b", "/srv/b", &[]));
        run_local(home.path(), &unpublish_script(a));
        let root = home.path().join(LAUNCH_ROOT);
        assert!(!root.join(project_dir_name(a)).exists());
        assert!(root.join(project_dir_name(b)).join("project").exists());
    }
}
