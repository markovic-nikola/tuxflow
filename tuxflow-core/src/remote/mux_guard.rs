//! Keeps the shared ssh connection to each host on a fast network path.
//!
//! Some routes pick a path PER TCP CONNECTION (ECMP hashing on the 5-tuple),
//! and when one of those paths is degraded, a random share of connections to
//! the same host lands on it. Measured 2026-10-05 against a Contabo VPS: 5 of
//! 16 fresh connections got a 105–160 ms minimum RTT with retransmits, the
//! rest 30–40 ms — while ping (one ICMP flow) showed a steady 37 ms. A
//! short-lived probe that draws the bad path costs a second; the
//! ControlMaster that drew it costs the whole day. Every multiplexed terminal
//! types through it, a lost packet stalls all of them at once, and it outlives
//! app restarts (ControlPersist plus any live client), so restarting TuxFlow
//! reattached to the same slow connection. That read as "remote agents are
//! laggy", with nothing wrong on the host or in the app.
//!
//! The kernel already knows each connection's base delay (`tcpi_min_rtt`, a
//! windowed minimum over five minutes, so queueing does not inflate it), and
//! `ss` reports it for every socket. Other connections to the same address —
//! MCP and mic forwards, tunnels, terminals that went direct — are free
//! samples of what the route can do; a connection is on a bad path when its
//! minimum is well above the best of them ([`is_slow`]).
//!
//! A slow master is replaced without the reconnect race: `-O stop` makes it
//! stop accepting clients (it keeps serving its sessions and unlinks the
//! control socket), a replacement is opened and kept only if it drew a fast
//! path, and only then is the old one terminated — its sessions exit 255,
//! and the app's reconnect (1 s later) reattaches their tmux sessions through
//! the new master. Killing first would have every terminal reconnect at once,
//! race for the socket, and the losers open direct connections that draw
//! paths at random all over again. Those direct terminals (`ssh -t` with our
//! ControlPath that could not multiplex) are checked the same way and sent
//! back through the master when they drew the slow path.
//!
//! [`rerouted_recently`] lets the app tell a deliberate move from an outage:
//! no "connection lost" notification for a reconnect the guard caused.

use super::{control_dir, ssh_mux_options, ssh_permit};
use std::collections::{BTreeSet, HashMap};
use std::process::{Command, Stdio};
use std::sync::{LazyLock, Mutex, Once};
use std::time::{Duration, Instant};

/// A connection is on a slow path when its minimum RTT is more than this
/// many times the best one to the same address...
const SLOW_RATIO: f64 = 2.0;
/// ...and more than this many ms above it — so a 3 ms LAN host with a 7 ms
/// flow is not "slow". Fast flows measured 26–36 ms against a 26 ms best
/// (ratio ≤ 1.4); slow ones 100–160 ms.
const SLOW_MARGIN_MS: f64 = 30.0;
/// Other connections needed before their best is trusted as the baseline;
/// below this the guard opens [`PROBES`] throwaway connections, once per
/// master.
const MIN_SAMPLES: usize = 2;
/// With ~30 % of paths bad, three probes all drawing one is ~3 %.
const PROBES: usize = 3;
/// Replacement masters tried before keeping whatever the last one drew.
const CANDIDATES: usize = 4;
const CHECK_INTERVAL: Duration = Duration::from_secs(30);
/// At most one intervention per host per window: each one blips the
/// terminals it moves, and a route that went bad for every flow at once
/// is not something reconnecting can fix.
const COOLDOWN: Duration = Duration::from_secs(180);
/// How long after an intervention a 255 counts as the guard's doing.
const REROUTE_GRACE: Duration = Duration::from_secs(15);

static HOSTS: LazyLock<Mutex<BTreeSet<String>>> = LazyLock::new(Default::default);
static REROUTED: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(Default::default);
static THREAD: Once = Once::new();

/// Keep `host`'s shared connection under watch. Non-blocking; starts the
/// guard thread on first use.
pub fn watch(host: &str) {
    HOSTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(host.to_string());
    THREAD.call_once(|| {
        std::thread::spawn(run);
    });
}

/// Whether the guard just moved `host`'s terminals — an exit 255 now is a
/// reroute, not an outage.
pub fn rerouted_recently(host: &str) -> bool {
    REROUTED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(host)
        .is_some_and(|t| t.elapsed() < REROUTE_GRACE)
}

fn mark_rerouted(host: &str) {
    REROUTED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(host.to_string(), Instant::now());
}

/// One established TCP connection as `ss` reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct Flow {
    /// Owning process — `None` for sockets of other users.
    pub pid: Option<u32>,
    /// Remote `addr:port`, as `ss` prints it.
    pub peer: String,
    pub min_rtt_ms: f64,
}

/// Parse `ss -tnpiH state established`: a socket line, then an indented
/// line of TCP info. Sockets without a `minrtt` yet are skipped.
pub fn parse_ss(text: &str) -> Vec<Flow> {
    let mut flows = Vec::new();
    let mut current: Option<(Option<u32>, String)> = None;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if !line.starts_with(char::is_whitespace) {
            let cols: Vec<&str> = line.split_whitespace().collect();
            // Recv-Q Send-Q Local Peer [Process]; a State column first when
            // no state filter was given.
            let skip = usize::from(cols.first().is_some_and(|c| c.parse::<u64>().is_err()));
            current = cols.get(3 + skip).map(|peer| {
                let pid = line.split("pid=").nth(1).and_then(|rest| {
                    rest.split(|c: char| !c.is_ascii_digit())
                        .next()?
                        .parse()
                        .ok()
                });
                (pid, peer.to_string())
            });
            continue;
        }
        let Some((pid, peer)) = current.take() else {
            continue;
        };
        let min_rtt = line
            .split_whitespace()
            .find_map(|tok| tok.strip_prefix("minrtt:"))
            .and_then(|v| v.parse::<f64>().ok());
        if let Some(min_rtt_ms) = min_rtt {
            flows.push(Flow {
                pid,
                peer,
                min_rtt_ms,
            });
        }
    }
    flows
}

/// Whether a connection with minimum RTT `candidate` sits on a worse path
/// than the best one seen to the same address (`baseline`).
pub fn is_slow(candidate: f64, baseline: f64) -> bool {
    candidate > baseline * SLOW_RATIO && candidate - baseline > SLOW_MARGIN_MS
}

/// The master's pid from `ssh -O check`'s "Master running (pid=N)".
pub fn parse_master_pid(stderr: &str) -> Option<u32> {
    stderr
        .split("pid=")
        .nth(1)?
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

/// Whether `argv` is one of our remote terminals that went direct: an
/// interactive `ssh -t` carrying our ControlPath. Probes have no `-t`;
/// forwards and tunnels use `ControlPath=none`.
pub fn is_terminal_client(argv: &[String], control_path_opt: &str) -> bool {
    argv.first().is_some_and(|a| a.ends_with("ssh"))
        && argv.iter().any(|a| a == "-t")
        && argv.iter().any(|a| a == control_path_opt)
}

fn control_path_opt() -> String {
    format!("ControlPath={}/ssh-%C", control_dir().to_string_lossy())
}

fn read_flows() -> Vec<Flow> {
    Command::new("ss")
        .args(["-tnpiH", "state", "established"])
        .stderr(Stdio::null())
        .output()
        .map(|out| parse_ss(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or_default()
}

fn argv_of(pid: u32) -> Vec<String> {
    std::fs::read(format!("/proc/{pid}/cmdline"))
        .map(|raw| {
            raw.split(|b| *b == 0)
                .filter(|a| !a.is_empty())
                .map(|a| String::from_utf8_lossy(a).into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// `ssh -O <op>` against `host`'s master; stderr is where ssh answers.
fn control(host: &str, op: &str) -> Option<String> {
    Command::new("ssh")
        .args(ssh_mux_options())
        .args(["-O", op, host])
        .stdin(Stdio::null())
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stderr).into_owned())
}

/// A process pinned by its start time, so a pid recycled between noticing
/// and killing is never hit.
#[derive(Clone, Copy)]
struct Proc {
    pid: u32,
    start: u64,
}

impl Proc {
    /// Identify `pid` NOW, while it is known to be ours — the master answered
    /// on our control socket, a terminal's argv was just checked. Later its
    /// title can no longer vouch: `-O stop` retitles a master "ssh: [stopped
    /// mux]", dropping the control path.
    fn of(pid: u32) -> Option<Proc> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        Some(Proc {
            pid,
            start: parse_start_time(&stat)?,
        })
    }

    /// SIGTERM, if it is still the same process.
    fn terminate(self) {
        if Proc::of(self.pid).is_some_and(|now| now.start == self.start) {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(self.pid as i32),
                nix::sys::signal::Signal::SIGTERM,
            );
        }
    }
}

/// Field 22 (`starttime`) of `/proc/<pid>/stat`, counted after the
/// parenthesised command name, which may itself hold spaces and parens.
pub fn parse_start_time(stat: &str) -> Option<u64> {
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

fn master(host: &str) -> Option<Proc> {
    Proc::of(parse_master_pid(&control(host, "check")?)?)
}

/// Minimum RTTs of a few throwaway connections — the baseline when nothing
/// else talks to the host.
fn probe(host: &str) -> Vec<f64> {
    let mut children: Vec<_> = (0..PROBES)
        .filter_map(|_| {
            Command::new("ssh")
                .args(["-o", "ControlMaster=no", "-o", "ControlPath=none"])
                .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"])
                .args([host, "sleep", "10"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .ok()
        })
        .collect();
    std::thread::sleep(Duration::from_secs(4));
    let pids: Vec<u32> = children.iter().map(|c| c.id()).collect();
    let samples = read_flows()
        .into_iter()
        .filter(|f| f.pid.is_some_and(|p| pids.contains(&p)))
        .map(|f| f.min_rtt_ms)
        .collect();
    for child in &mut children {
        let _ = child.kill();
        let _ = child.wait();
    }
    samples
}

#[derive(Default)]
struct HostState {
    last_action: Option<Instant>,
    /// The master the probes already ran for — once per master is enough.
    probed: Option<u32>,
}

fn run() {
    let mut states: HashMap<String, HostState> = HashMap::new();
    loop {
        std::thread::sleep(CHECK_INTERVAL);
        let hosts: Vec<String> = HOSTS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect();
        for host in hosts {
            check(&host, states.entry(host.clone()).or_default());
        }
    }
}

/// One pass over `host` right now, outside the guard thread's schedule —
/// for `examples/mux_guard_check.rs`. Blocking.
pub fn check_now(host: &str) {
    check(host, &mut HostState::default());
}

fn check(host: &str, state: &mut HostState) {
    if state.last_action.is_some_and(|t| t.elapsed() < COOLDOWN) {
        return;
    }
    let Some(master_proc) = master(host) else {
        return;
    };
    let master = master_proc.pid;
    let flows = read_flows();
    // No TCP socket of its own = a ProxyCommand/ProxyJump master, whose
    // path belongs to the proxy.
    let Some(master_flow) = flows.iter().find(|f| f.pid == Some(master)).cloned() else {
        return;
    };
    let mut samples: Vec<f64> = flows
        .iter()
        .filter(|f| f.peer == master_flow.peer && f.pid != Some(master))
        .map(|f| f.min_rtt_ms)
        .collect();
    if samples.len() < MIN_SAMPLES && state.probed != Some(master) {
        state.probed = Some(master);
        samples.extend(probe(host));
    }
    let Some(baseline) = samples.into_iter().reduce(f64::min) else {
        return;
    };

    if is_slow(master_flow.min_rtt_ms, baseline) {
        log::warn!(
            "ssh to {host}: shared connection on a slow path ({:.0} ms min RTT, best {:.0} ms) — replacing it",
            master_flow.min_rtt_ms,
            baseline
        );
        state.last_action = Some(Instant::now());
        reroute(host, master_proc, baseline);
        return;
    }

    let opt = control_path_opt();
    let stragglers: Vec<Proc> = flows
        .iter()
        .filter(|f| f.peer == master_flow.peer && is_slow(f.min_rtt_ms, baseline))
        .filter_map(|f| f.pid)
        .filter(|p| is_terminal_client(&argv_of(*p), &opt))
        .filter_map(Proc::of)
        .collect();
    if stragglers.is_empty() {
        return;
    }
    log::warn!(
        "ssh to {host}: {} terminal(s) on a slow path (best {:.0} ms) — moving them to the shared connection",
        stragglers.len(),
        baseline
    );
    state.last_action = Some(Instant::now());
    mark_rerouted(host);
    for proc in stragglers {
        proc.terminate();
    }
}

/// Swap `old` for a master on a fast path, then retire `old` (see the
/// module docs for why in that order).
fn reroute(host: &str, old: Proc, baseline: f64) {
    if control(host, "stop").is_none() {
        return;
    }
    let mut replaced = false;
    for attempt in 1..=CANDIDATES {
        {
            let _permit = ssh_permit();
            // With the old socket gone this ssh becomes the new master
            // (ControlPersist forks it into the background) — or joins one
            // an app probe opened in the meantime; either is evaluated.
            let _ = Command::new("ssh")
                .args(ssh_mux_options())
                .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"])
                .args([host, "true"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let Some(new) = master(host).filter(|p| p.pid != old.pid) else {
            continue;
        };
        let Some(rtt) = read_flows()
            .into_iter()
            .find(|f| f.pid == Some(new.pid))
            .map(|f| f.min_rtt_ms)
        else {
            // Through a proxy after all — nothing to compare.
            replaced = true;
            break;
        };
        replaced = true;
        if !is_slow(rtt, baseline) || attempt == CANDIDATES {
            log::info!("ssh to {host}: new shared connection at {rtt:.0} ms (attempt {attempt})");
            break;
        }
        log::info!("ssh to {host}: replacement drew the slow path too ({rtt:.0} ms), retrying");
        new.terminate();
        std::thread::sleep(Duration::from_millis(200));
    }
    // No replacement (host unreachable right now): the old master keeps its
    // sessions, and the next client opens a master of its own.
    if replaced {
        mark_rerouted(host);
        old.terminate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SS: &str = "\
0      0      192.168.0.22:60504 161.97.151.195:22 users:((\"ssh\",pid=5540,fd=3))
\t cubic wscale:7,7 rto:470 rtt:189.335/47.683 ato:62 mss:1448 minrtt:129.383 snd_wnd:536320
0      0      192.168.0.22:56344 161.97.151.195:22 users:((\"ssh\",pid=323477,fd=3))
\t ts sack cubic rtt:36.003/2.186 minrtt:25.802 rcv_ooopack:1
0      0      192.168.0.22:41000 140.82.121.3:22
\t ts sack cubic rtt:41.1/2.5 minrtt:35.824
0      0      192.168.0.22:41001 161.97.151.195:22 users:((\"ssh\",pid=9,fd=3))
\t ts sack cubic rtt:41.1/2.5
";

    #[test]
    fn ss_output_parses_into_flows() {
        let flows = parse_ss(SS);
        assert_eq!(flows.len(), 3, "the socket without minrtt is skipped");
        assert_eq!(
            flows[0],
            Flow {
                pid: Some(5540),
                peer: "161.97.151.195:22".into(),
                min_rtt_ms: 129.383
            }
        );
        // A socket of another user carries no process column.
        assert_eq!(flows[2].pid, None);
        assert_eq!(flows[2].peer, "140.82.121.3:22");
    }

    #[test]
    fn ss_with_a_state_column_parses_too() {
        let flows = parse_ss(
            "ESTAB 0 0 [::1]:5000 [2a02::5]:22 users:((\"ssh\",pid=7,fd=3))\n\t minrtt:12.5\n",
        );
        assert_eq!(flows[0].peer, "[2a02::5]:22");
        assert_eq!(flows[0].pid, Some(7));
    }

    #[test]
    fn slow_means_both_twice_and_well_above_the_best() {
        // The measured case: 129 ms master against 26 ms elsewhere.
        assert!(is_slow(129.0, 26.0));
        // Ordinary spread between fast flows.
        assert!(!is_slow(36.0, 26.0));
        // A LAN host: triple the best, but only a few ms in absolute terms.
        assert!(!is_slow(1.0, 0.3));
        assert!(!is_slow(40.0, 15.0), "ratio alone is not enough");
    }

    #[test]
    fn start_time_survives_a_command_name_with_spaces_and_parens() {
        let stat = "5540 (ssh: /run/x [mux) )) S 1 5540 5540 0 -1 4194624 \
                    1 0 0 0 10 20 0 0 20 0 1 0 987654 1000 200";
        assert_eq!(parse_start_time(stat), Some(987654));
    }

    #[test]
    fn master_pid_from_check_output() {
        assert_eq!(
            parse_master_pid("Master running (pid=5540)\r\n"),
            Some(5540)
        );
        assert_eq!(
            parse_master_pid("Control socket connect(x): No such file"),
            None
        );
    }

    #[test]
    fn only_our_interactive_ssh_counts_as_a_terminal() {
        let opt = "ControlPath=/run/user/1000/tuxflow/ssh-%C";
        let argv = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        assert!(is_terminal_client(
            &argv(&format!("ssh -t -o LogLevel=ERROR -o {opt} my-server cd x")),
            opt
        ));
        // A probe over the same ControlPath: no -t.
        assert!(!is_terminal_client(
            &argv(&format!("ssh -o {opt} my-server git status")),
            opt
        ));
        // The user's own interactive ssh: not ours.
        assert!(!is_terminal_client(&argv("ssh -t my-server"), opt));
    }
}
