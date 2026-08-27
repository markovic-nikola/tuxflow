//! Lines TuxFlow writes into a process terminal itself.
//!
//! A process keeps ONE terminal for its whole lifetime in both shells — GTK
//! reuses a single VTE widget, the iced shell respawns into the same
//! alacritty grid — so a terminal is the process's log across runs, not just
//! the current one. These are the markers that make such a log readable: why
//! a run ended, and where the next one begins. Shared so the wording can't
//! drift between the shells.

/// Explain a bad exit INSIDE the terminal. Crucial for remote processes: an
/// error printed inside the tmux pane (e.g. the shell's "command not found")
/// vanishes with the session, leaving only tmux's bare "[exited]" behind.
///
/// `None` for a clean exit — a run that ended the way it was asked to needs
/// no explanation, and the status pill already says it stopped.
///
/// Ready to feed: the CRLFs that put it on a line of its own are included.
pub fn exit_banner(
    status: i32,
    connection_loss: bool,
    command: &str,
    host: Option<&str>,
) -> Option<String> {
    if status == 0 {
        return None;
    }
    let msg = if connection_loss {
        // Exit 255 is ssh's own "link died", not the command's — the run is
        // still alive in its tmux session on the host.
        String::from(
            "\x1b[1;33m[tuxflow]\x1b[0m connection lost — reconnecting, the process keeps \
             running on the host",
        )
    } else if status == 127 || status == 126 {
        let what = if status == 127 {
            "command not found"
        } else {
            "command not executable"
        };
        let cmd = ellipsize(command, 60);
        match host {
            Some(host) => format!(
                "\x1b[1;31m[tuxflow]\x1b[0m exit {status} — {what} on {host}: \
                 \x1b[1m{cmd}\x1b[0m (is it installed there?)"
            ),
            None => format!("\x1b[1;31m[tuxflow]\x1b[0m exit {status} — {what}: {cmd}"),
        }
    } else {
        format!("\x1b[1;31m[tuxflow]\x1b[0m process exited with status {status}")
    };
    Some(format!("\r\n{msg}\r\n"))
}

/// The dim rule a new run starts under, so two runs in one terminal read as
/// two runs. `cols` is the terminal's current width; the rule stops one
/// column short of it, since filling the last cell leaves the grid in a
/// pending-wrap state that the following newline has to spend.
///
/// Ready to feed, like [`exit_banner`].
pub fn run_separator(cols: usize, label: &str) -> String {
    let label = format!(" {label} ");
    let len = label.chars().count();
    let width = cols.saturating_sub(1).max(len + 8);
    let left = (width - len) / 2;
    let right = width - len - left;
    format!(
        "\r\n\x1b[2m{}{label}{}\x1b[0m\r\n",
        "─".repeat(left),
        "─".repeat(right)
    )
}

/// Truncate on a CHARACTER boundary — `String::truncate` panics mid-glyph,
/// and a command line is exactly where a non-ASCII byte turns up.
fn ellipsize(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_exit_says_nothing() {
        assert_eq!(exit_banner(0, false, "npm run dev", None), None);
    }

    #[test]
    fn missing_command_names_the_host_and_the_command() {
        let msg = exit_banner(127, false, "bun run dev", Some("my-server")).expect("banner");
        assert!(msg.contains("command not found"));
        assert!(msg.contains("my-server"));
        assert!(msg.contains("bun run dev"));
        // Local runs have no host to blame.
        let local = exit_banner(127, false, "bun run dev", None).expect("banner");
        assert!(!local.contains("is it installed there?"));
    }

    #[test]
    fn connection_loss_is_not_a_crash() {
        let msg = exit_banner(255, true, "php artisan dev", Some("my-server")).expect("banner");
        assert!(msg.contains("connection lost"));
        assert!(msg.contains("keeps running on the host"));
        // The command's own exit 255 still reads as a crash.
        let crash = exit_banner(255, false, "php artisan dev", None).expect("banner");
        assert!(crash.contains("status 255"));
    }

    #[test]
    fn banner_sits_on_its_own_line() {
        let msg = exit_banner(1, false, "x", None).expect("banner");
        assert!(msg.starts_with("\r\n") && msg.ends_with("\r\n"));
    }

    #[test]
    fn long_multibyte_command_truncates_without_panicking() {
        let cmd = "ssh my-server 'echo ✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓✓'";
        let msg = exit_banner(126, false, cmd, None).expect("banner");
        assert!(msg.contains('…'));
    }

    #[test]
    fn separator_fills_the_width_it_is_given() {
        let sep = run_separator(40, "restarted");
        let rule = sep.trim_matches(|c| c == '\r' || c == '\n');
        let rule = rule.replace("\x1b[2m", "").replace("\x1b[0m", "");
        assert_eq!(rule.chars().count(), 39, "one short of the last column");
        assert!(rule.contains(" restarted "));
    }

    #[test]
    fn separator_survives_a_terminal_too_narrow_for_its_label() {
        let sep = run_separator(0, "reconnecting");
        assert!(sep.contains(" reconnecting "));
    }
}
