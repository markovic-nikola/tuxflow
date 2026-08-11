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
    let out = std::process::Command::new("ssh")
        .args(ssh_mux_options())
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"])
        .arg(host)
        .arg(build_script(sessions))
        .output();
    match out {
        Ok(o) => parse(&String::from_utf8_lossy(&o.stdout), sessions),
        Err(e) => {
            log::warn!("remote port probe on {host} failed to run ssh: {e}");
            HashMap::new()
        }
    }
}

/// Collect the three tables the join needs — pane pid per session, the process
/// table, and every listening socket — in one round trip. Deliberately no
/// filtering here: matching pids to ports in shell would be unreadable and
/// untestable, and these tables are small.
fn build_script(sessions: &[String]) -> String {
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
    s.push_str("ps -eo pid=,ppid= | sed 's/^ *//; s/^/proc /'; ");
    s.push_str("ss -ltnpH 2>/dev/null | sed 's/^/sock /'; true");
    s
}

/// Join the tables: descendants of each pane pid, then the ports they listen
/// on. `sessions` resolves the index labels the script emits back to names.
fn parse(out: &str, sessions: &[String]) -> HashMap<String, Vec<u16>> {
    let mut panes: Vec<(String, i32)> = Vec::new();
    let mut children: HashMap<i32, Vec<i32>> = HashMap::new();
    // pid -> ports it listens on
    let mut listeners: Vec<(i32, u16)> = Vec::new();

    for line in out.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("pane ") {
            // `pane <index> <pid>`
            if let Some((idx, pid)) = rest.split_once(' ')
                && let (Ok(idx), Ok(pid)) = (idx.trim().parse::<usize>(), pid.trim().parse::<i32>())
                && let Some(session) = sessions.get(idx)
            {
                panes.push((session.clone(), pid));
            }
        } else if let Some(rest) = line.strip_prefix("proc ") {
            let mut f = rest.split_whitespace();
            if let (Some(Ok(pid)), Some(Ok(ppid))) = (
                f.next().map(str::parse::<i32>),
                f.next().map(str::parse::<i32>),
            ) {
                children.entry(ppid).or_default().push(pid);
            }
        } else if let Some(rest) = line.strip_prefix("sock ") {
            let f: Vec<&str> = rest.split_whitespace().collect();
            // LISTEN 0 4096 127.0.0.1:8000 0.0.0.0:* users:(("php",pid=12,fd=6))
            let Some(local) = f.get(3) else { continue };
            let Some(port) = local.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()) else {
                continue;
            };
            // A socket with no owning pid belongs to another user — not ours.
            for pid in rest.match_indices("pid=").filter_map(|(i, _)| {
                rest[i + 4..]
                    .split(|c: char| !c.is_ascii_digit())
                    .next()
                    .and_then(|d| d.parse::<i32>().ok())
            }) {
                listeners.push((pid, port));
            }
        }
    }

    let mut result = HashMap::new();
    for (session, pane_pid) in panes {
        let tree = descendants(pane_pid, &children);
        let mut ports: Vec<u16> = listeners
            .iter()
            .filter(|(pid, _)| tree.contains(pid))
            .map(|(_, port)| *port)
            .collect();
        ports.sort_unstable();
        ports.dedup();
        result.insert(session, ports);
    }
    result
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
    use super::{build_script, parse};

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
        let s = build_script(&sessions(&["tf-dev-1"]));
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

    #[test]
    fn script_quotes_session_names() {
        let s = build_script(&["tf-dev-1; rm -rf /".to_string()]);
        assert!(!s.contains("; rm -rf /;"));
        assert!(s.contains(r"'tf-dev-1; rm -rf /'"));
    }

    #[test]
    fn script_collects_every_session_in_one_round_trip() {
        let s = build_script(&["a".to_string(), "b".to_string()]);
        assert_eq!(s.matches("list-panes").count(), 2);
        // The heavy tables are fetched once, not per session.
        assert_eq!(s.matches("ps -eo").count(), 1);
        assert_eq!(s.matches("ss -ltnpH").count(), 1);
    }
}
