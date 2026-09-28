//! Port discovery for remote projects, from the process tree rather than from
//! terminal output.
//!
//! `PortDetector` can only know what a process *prints*, and full-screen TUI
//! runners print almost nothing: `php artisan dev` draws `@laravel/multiplex`,
//! a tabbed panel where only the selected tab is rendered. A run parked on the
//! `vite` tab never shows its server URL, so the port is undetectable — not
//! wrapped, not truncated, simply absent — and on a remote project that means
//! no tunnel and a dead browser button, for a server that is up and serving.
//!
//! Asking the host what the run is *listening on* sidesteps the whole class:
//! it is true regardless of which tab is drawn, which runner is used, or
//! whether anything was ever printed. Output scanning stays, but only for what
//! it is actually good at — judging which port is the app's user-facing URL.
//!
//! Ports found here are forwarded 1:1 (see `TunnelManager::ensure_exact`),
//! because a remote dev server hands the browser its own address: Vite's
//! `public/hot` and Laravel's `APP_URL` both name a port this machine must
//! then be able to reach under that exact number.

use crate::remote::{TMUX_SOCKET, sh_quote, ssh_mux_options};
use std::collections::{HashMap, HashSet};

/// Listening TCP ports per tmux session, for every session that still exists.
///
/// Blocking: one ssh round trip. Call from a worker thread, never the GTK
/// main loop.
pub fn session_ports(host: &str, sessions: &[String]) -> HashMap<String, Vec<u16>> {
    if sessions.is_empty() {
        return HashMap::new();
    }
    match run_script(host, &build_script(sessions, false)) {
        Some(out) => parse(&out, sessions),
        None => HashMap::new(),
    }
}

/// What one run's process tree looks like from the host, for deciding
/// whether the page it serves can be opened yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunListeners {
    /// Every TCP port the run's processes listen on.
    pub ports: Vec<u16>,
    /// A Vite process is in the tree but nothing under it listens yet. The
    /// app server beside it (`php artisan serve` under `php artisan dev`)
    /// is up seconds earlier, and a page loaded in between references a
    /// dev server that is not there: no stylesheets, dead script tags.
    pub vite_starting: bool,
}

/// The listeners of one run, or None when its session has no live pane or
/// the host could not be asked (a host without tmux leaves no tree to walk).
///
/// Blocking: one ssh round trip. Call from a worker thread.
pub fn run_listeners(host: &str, session: &str) -> Option<RunListeners> {
    let sessions = [session.to_string()];
    let out = run_script(host, &build_script(&sessions, true))?;
    Snapshot::parse(&out, &sessions).run_listeners(session)
}

fn run_script(host: &str, script: &str) -> Option<String> {
    let out = std::process::Command::new("ssh")
        .args(ssh_mux_options())
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"])
        .arg(host)
        .arg(script)
        .output();
    match out {
        Ok(o) => Some(String::from_utf8_lossy(&o.stdout).into_owned()),
        Err(e) => {
            log::warn!("remote port probe on {host} failed to run ssh: {e}");
            None
        }
    }
}

/// Collect the three tables the join needs — pane pid per session, the process
/// table, and every listening socket — in one round trip. Deliberately no
/// filtering here: matching pids to ports in shell would be unreadable and
/// untestable, and these tables are small. `with_args` adds each process's
/// command line, which only the readiness probe needs — the periodic poll
/// would otherwise haul the host's whole argv table every tick.
fn build_script(sessions: &[String], with_args: bool) -> String {
    let mut s = String::new();
    for (i, session) in sessions.iter().enumerate() {
        // Label the reply with the session's *index*, never its name: the name
        // has to be shell-quoted to reach tmux safely, and those quotes are
        // literal inside the echo — the label would come back wearing them and
        // match nothing. An index also sidesteps names containing spaces.
        s.push_str(&format!(
            "p=$(tmux -L {TMUX_SOCKET} list-panes -t {q} -F '#{{pane_pid}}' 2>/dev/null | head -1); \
             [ -n \"$p\" ] && echo \"pane {i} $p\"; ",
            q = sh_quote(session)
        ));
    }
    // `ps` gives the parent links: multiplex starts each child in a session of
    // its own (setsid), so descendants are NOT reachable by session id — only
    // the ppid chain reaches them.
    let columns = if with_args {
        "pid=,ppid=,args="
    } else {
        "pid=,ppid="
    };
    s.push_str(&format!("ps -eo {columns} | sed 's/^ *//; s/^/proc /'; "));
    s.push_str("ss -ltnpH 2>/dev/null | sed 's/^/sock /'; true");
    s
}

/// The script's reply, parsed but not yet joined.
#[derive(Default)]
struct Snapshot {
    panes: Vec<(String, i32)>,
    children: HashMap<i32, Vec<i32>>,
    /// pid -> command line (only when the script asked for it)
    args: HashMap<i32, String>,
    /// pid -> ports it listens on
    listeners: Vec<(i32, u16)>,
}

impl Snapshot {
    /// `sessions` resolves the index labels the script emits back to names.
    fn parse(out: &str, sessions: &[String]) -> Self {
        let mut snap = Self::default();
        for line in out.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("pane ") {
                // `pane <index> <pid>`
                if let Some((idx, pid)) = rest.split_once(' ')
                    && let (Ok(idx), Ok(pid)) =
                        (idx.trim().parse::<usize>(), pid.trim().parse::<i32>())
                    && let Some(session) = sessions.get(idx)
                {
                    snap.panes.push((session.clone(), pid));
                }
            } else if let Some(rest) = line.strip_prefix("proc ") {
                // `proc <pid> <ppid>[ <args…>]` — ps pads the columns
                let (pid, rest) = next_field(rest);
                let (ppid, args) = next_field(rest);
                if let (Ok(pid), Ok(ppid)) = (pid.parse::<i32>(), ppid.parse::<i32>()) {
                    snap.children.entry(ppid).or_default().push(pid);
                    if !args.is_empty() {
                        snap.args.insert(pid, args.to_string());
                    }
                }
            } else if let Some(rest) = line.strip_prefix("sock ") {
                let f: Vec<&str> = rest.split_whitespace().collect();
                // LISTEN 0 4096 127.0.0.1:8000 0.0.0.0:* users:(("php",pid=12,fd=6))
                let Some(local) = f.get(3) else { continue };
                let Some(port) = local.rsplit(':').next().and_then(|p| p.parse::<u16>().ok())
                else {
                    continue;
                };
                // A socket with no owning pid belongs to another user — not ours.
                for pid in rest.match_indices("pid=").filter_map(|(i, _)| {
                    rest[i + 4..]
                        .split(|c: char| !c.is_ascii_digit())
                        .next()
                        .and_then(|d| d.parse::<i32>().ok())
                }) {
                    snap.listeners.push((pid, port));
                }
            }
        }
        snap
    }

    fn tree(&self, session: &str) -> Option<HashSet<i32>> {
        let &(_, pane_pid) = self.panes.iter().find(|(s, _)| s == session)?;
        Some(descendants(pane_pid, &self.children))
    }

    fn ports_of(&self, pids: &HashSet<i32>) -> Vec<u16> {
        let mut ports: Vec<u16> = self
            .listeners
            .iter()
            .filter(|(pid, _)| pids.contains(pid))
            .map(|(_, port)| *port)
            .collect();
        ports.sort_unstable();
        ports.dedup();
        ports
    }

    fn run_listeners(&self, session: &str) -> Option<RunListeners> {
        let tree = self.tree(session)?;
        // Each Vite is judged by its SUBTREE: `sh -c vite` never listens
        // itself, the node under it does, and a wrapper script named `vite`
        // may exec or fork into something that no longer says vite at all.
        let vite_starting = tree
            .iter()
            .filter(|pid| self.args.get(pid).is_some_and(|a| is_vite_server(a)))
            .any(|&pid| self.ports_of(&descendants(pid, &self.children)).is_empty());
        Some(RunListeners {
            ports: self.ports_of(&tree),
            vite_starting,
        })
    }
}

/// Split off the first whitespace-delimited field, returning it and the
/// rest with its leading padding removed.
fn next_field(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    match s.split_once(char::is_whitespace) {
        Some((field, rest)) => (field, rest.trim_start()),
        None => (s, ""),
    }
}

/// A command line that runs Vite's dev server — `vite`, `sh -c vite`,
/// `node …/node_modules/.bin/vite`, `node …/vite/bin/vite.js` — but not
/// `vite build`, which never listens and would hold the open until the cap.
fn is_vite_server(args: &str) -> bool {
    let mut words = args.split_whitespace();
    let Some(at) = words.position(|w| {
        let base = w.rsplit('/').next().unwrap_or(w);
        base == "vite" || base == "vite.js"
    }) else {
        return false;
    };
    !args.split_whitespace().skip(at + 1).any(|w| w == "build")
}

/// Join the tables: descendants of each pane pid, then the ports they listen
/// on.
fn parse(out: &str, sessions: &[String]) -> HashMap<String, Vec<u16>> {
    let snap = Snapshot::parse(out, sessions);
    snap.panes
        .iter()
        .map(|(session, pane_pid)| {
            let tree = descendants(*pane_pid, &snap.children);
            (session.clone(), snap.ports_of(&tree))
        })
        .collect()
}

/// `root` and everything below it in the ppid graph.
fn descendants(root: i32, children: &HashMap<i32, Vec<i32>>) -> HashSet<i32> {
    let mut seen = HashSet::from([root]);
    let mut queue = vec![root];
    while let Some(pid) = queue.pop() {
        for &child in children.get(&pid).into_iter().flatten() {
            // Guard against a cycle in a torn process table rather than trust it.
            if seen.insert(child) {
                queue.push(child);
            }
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::{RunListeners, Snapshot, build_script, is_vite_server, parse};

    /// Shapes and pid relationships taken from a real `php artisan dev` run:
    /// the pane shell, `sh -c npx`, npm, `sh -c multiplex`, node/multiplex,
    /// then each server under its own `sh -c`.
    const REAL: &str = "\
pane 0 1573520
proc 1573520 956832
proc 1573522 1573520
proc 1573533 1573522
proc 1573549 1573533
proc 1573550 1573549
proc 1573561 1573550
proc 1573583 1573561
proc 1573792 1573550
proc 1573793 1573792
proc 999 1
sock LISTEN 0      4096   127.0.0.1:8000 0.0.0.0:* users:((\"php8.4\",pid=1573583,fd=6))
sock LISTEN 0      511    127.0.0.1:5173 0.0.0.0:* users:((\"node\",pid=1573793,fd=21))
sock LISTEN 0      200    127.0.0.1:5432 0.0.0.0:*
sock LISTEN 0      4096   0.0.0.0:22 0.0.0.0:* users:((\"sshd\",pid=999,fd=3))
";

    fn sessions(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn finds_ports_of_the_whole_descendant_tree() {
        let got = parse(REAL, &sessions(&["tf-dev-9ee1daac"]));
        // 8000 is six levels below the pane, 5173 on a sibling branch.
        assert_eq!(got.get("tf-dev-9ee1daac"), Some(&vec![5173, 8000]));
    }

    #[test]
    fn ignores_sockets_outside_the_tree() {
        let got = parse(REAL, &sessions(&["tf-dev-9ee1daac"]));
        let ports = &got["tf-dev-9ee1daac"];
        // postgres has no owning pid here; sshd's pid is not a descendant.
        assert!(!ports.contains(&5432), "{ports:?}");
        assert!(!ports.contains(&22), "{ports:?}");
    }

    #[test]
    fn labels_are_indices_so_quoting_cannot_corrupt_them() {
        // The script must not echo the session *name*: reaching tmux safely
        // requires shell-quoting it, and those quotes are literal inside the
        // echo, so the label returns as `'tf-dev-1'` and matches nothing.
        let s = build_script(&sessions(&["tf-dev-1"]), false);
        assert!(s.contains("echo \"pane 0 $p\""), "{s}");
        assert!(!s.contains("echo \"pane 'tf-dev-1'"), "{s}");
    }

    #[test]
    fn indices_map_back_to_the_right_session() {
        let out = "\
pane 1 20
proc 20 1
proc 21 20
sock LISTEN 0 128 127.0.0.1:8001 0.0.0.0:* users:((\"php\",pid=21,fd=4))
";
        let got = parse(out, &sessions(&["first", "second"]));
        assert_eq!(got.get("second"), Some(&vec![8001]));
        assert!(!got.contains_key("first"));
    }

    #[test]
    fn out_of_range_index_ignored() {
        // A reply that cannot be attributed is dropped, not misattributed.
        let got = parse("pane 7 20\nproc 20 1\n", &sessions(&["only"]));
        assert!(got.is_empty());
    }

    #[test]
    fn session_with_no_live_pane_is_absent() {
        // `[ -n "$p" ]` means a dead session emits no `pane` line at all —
        // absent, not empty, so a caller can tell "gone" from "nothing bound".
        assert!(parse("proc 1 0\n", &sessions(&["gone"])).is_empty());
    }

    #[test]
    fn ipv6_listener_port_parsed() {
        let out = "\
pane 0 10
proc 10 1
proc 11 10
sock LISTEN 0 128 [::1]:8001 [::]:* users:((\"php\",pid=11,fd=4))
";
        assert_eq!(parse(out, &sessions(&["s"])).get("s"), Some(&vec![8001]));
    }

    #[test]
    fn cyclic_process_table_terminates() {
        // A torn `ps` snapshot can imply a cycle; it must not hang the probe.
        let out = "\
pane 0 10
proc 10 11
proc 11 10
sock LISTEN 0 128 127.0.0.1:9000 0.0.0.0:* users:((\"x\",pid=11,fd=4))
";
        assert_eq!(parse(out, &sessions(&["s"])).get("s"), Some(&vec![9000]));
    }

    /// `php artisan dev` with command lines, the way the readiness probe
    /// sees it: multiplex, the PHP server, and npm → `sh -c vite` → node.
    const WITH_ARGS: &str = "\
pane 0 100
proc 100 1 -zsh
proc 101 100 node /usr/bin/npx multiplex
proc 102 101 sh -c php artisan serve
proc 103 102 php8.4 artisan serve
proc 104 103 /usr/bin/php8.4 -S 127.0.0.1:8000 /srv/app/vendor/laravel/framework/server.php
proc 105 101 sh -c npm run dev
proc 106 105 npm run dev
proc 107 106 sh -c vite
proc 108 107 node /srv/app/node_modules/.bin/vite
proc 200 1 node /srv/other/node_modules/.bin/vite
sock LISTEN 0 4096 127.0.0.1:8000 0.0.0.0:* users:((\"php8.4\",pid=104,fd=4))
";

    fn listeners(out: &str) -> Option<RunListeners> {
        Snapshot::parse(out, &sessions(&["s"])).run_listeners("s")
    }

    #[test]
    fn vite_in_the_tree_without_a_socket_is_starting() {
        // The app server is up, Vite is not: this is the moment the page
        // used to open unstyled.
        let got = listeners(WITH_ARGS).unwrap();
        assert_eq!(got.ports, vec![8000]);
        assert!(got.vite_starting);
    }

    #[test]
    fn vite_listening_is_not_starting() {
        let out = format!(
            "{WITH_ARGS}sock LISTEN 0 511 [::1]:5173 [::]:* users:((\"node\",pid=108,fd=21))\n"
        );
        let got = listeners(&out).unwrap();
        assert_eq!(got.ports, vec![5173, 8000]);
        assert!(!got.vite_starting);
    }

    #[test]
    fn another_projects_vite_does_not_count() {
        // pid 200 is a Vite outside this run's tree, listening: it must not
        // make this run's own (still silent) Vite look ready.
        let out = format!(
            "{WITH_ARGS}sock LISTEN 0 511 127.0.0.1:5174 0.0.0.0:* users:((\"node\",pid=200,fd=21))\n"
        );
        let got = listeners(&out).unwrap();
        assert_eq!(got.ports, vec![8000]);
        assert!(got.vite_starting);
    }

    #[test]
    fn vite_counts_as_listening_through_its_children() {
        // A `vite` wrapper whose listener is a child with another name —
        // judged by the Vite's subtree, not by the socket's owner alone.
        let out = "\
pane 0 10
proc 10 1 -zsh
proc 11 10 /bin/sh /srv/app/bin/vite
proc 12 11 python3 -m http.server 5391
sock LISTEN 0 5 127.0.0.1:5391 0.0.0.0:* users:((\"python3\",pid=12,fd=3))
";
        assert!(!listeners(out).unwrap().vite_starting);
    }

    #[test]
    fn run_without_vite_is_never_starting() {
        let out = "\
pane 0 10
proc 10 1 -zsh
proc 11 10 php artisan serve
sock LISTEN 0 128 127.0.0.1:8000 0.0.0.0:* users:((\"php\",pid=11,fd=4))
";
        assert_eq!(
            listeners(out),
            Some(RunListeners {
                ports: vec![8000],
                vite_starting: false
            })
        );
    }

    #[test]
    fn dead_session_has_no_listeners() {
        assert_eq!(listeners("proc 1 0 init\n"), None);
    }

    #[test]
    fn vite_command_lines() {
        assert!(is_vite_server("vite"));
        assert!(is_vite_server("sh -c vite"));
        assert!(is_vite_server(
            "node /srv/app/node_modules/.bin/vite --host"
        ));
        assert!(is_vite_server(
            "node /srv/app/node_modules/vite/bin/vite.js"
        ));
        assert!(!is_vite_server("vite build --watch"));
        assert!(!is_vite_server("npm run dev"));
        assert!(!is_vite_server("node /srv/app/node_modules/.bin/vitest"));
        assert!(!is_vite_server("vim vite.config.js"));
    }

    #[test]
    fn readiness_script_asks_for_command_lines_and_the_poll_does_not() {
        let names = sessions(&["s"]);
        assert!(build_script(&names, true).contains("ps -eo pid=,ppid=,args="));
        assert!(build_script(&names, false).contains("ps -eo pid=,ppid= |"));
    }

    #[test]
    fn script_quotes_session_names() {
        let s = build_script(&["tf-dev-1; rm -rf /".to_string()], false);
        assert!(!s.contains("; rm -rf /;"));
        assert!(s.contains(r"'tf-dev-1; rm -rf /'"));
    }

    #[test]
    fn script_collects_every_session_in_one_round_trip() {
        let s = build_script(&["a".to_string(), "b".to_string()], false);
        assert_eq!(s.matches("list-panes").count(), 2);
        // The heavy tables are fetched once, not per session.
        assert_eq!(s.matches("ps -eo").count(), 1);
        assert_eq!(s.matches("ss -ltnpH").count(), 1);
    }
}
