//! Is an agent stuck waiting on its API rather than working?
//!
//! While Claude Code retries a request it keeps repainting a countdown,
//! so the repaint signal in `activity` reads it as busy — the card
//! breathes and the dot's arc turns for an agent that is going nowhere.
//! This reads the retry line itself off the grid instead.
//!
//! Every retry state is one Claude Code component (verified against
//! 2.1.281), and each line it draws opens with the `✻` mark followed by
//! a reason and a dim ` · `-separated tail:
//!
//! - `✻ Waiting for API response · will retry in 2m 34s · check your network`
//! - `✻ <error> · Retrying in 5s · attempt 2/10` (the error may be
//!   "Usage limit reached", with ` (resets …)` after the wait)
//! - `✻ No response from the API after 45s · retrying, waiting up to 2m · attempt 1/10`
//! - `✻ <text> · next try in 30s · attempt 3 · esc to interrupt`
//!
//! The mark in COLUMN 0 is what makes the match safe. Claude Code draws
//! the line with `\r` and then the mark (captured from a real 2.1.281
//! pointed at a closed port), while everything it prints in the
//! conversation is indented under a `⏺` — so an agent discussing these
//! lines, or quoting one, never puts `✻` first on a row. The normal
//! spinner also cycles through `✻`, but its tail never carries a retry
//! phrase.

/// The mark every retry line opens with (Claude Code's `OT`).
const MARK: char = '\u{273B}';
/// Separator between the line's fields.
const SEP: &str = " \u{00B7} ";

/// The tail phrases that make a `✻` line a retry line, each followed by
/// the wait it announces.
const RETRY_PHRASES: &[&str] = &[
    "will retry in ",
    "Retrying in ",
    "next try in ",
    "retrying, waiting up to ",
    "retrying once, waiting up to ",
];

/// What the agent is stuck on, as its own retry line says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stall {
    /// The line's leading field: "Waiting for API response", the
    /// formatted error, "Usage limit reached", …
    pub reason: String,
    /// The wait before the next try, as printed ("2m 34s", "5s"); an
    /// upper bound ("≤2m") for the no-response form.
    pub wait: Option<String>,
}

/// The newest retry line in `text` (the rows above an agent's cursor),
/// if there is one. Bottom-up, so a line still on screen from an earlier
/// stall never outranks the current one.
pub fn detect(text: &str) -> Option<Stall> {
    text.lines().rev().find_map(parse_line)
}

fn parse_line(line: &str) -> Option<Stall> {
    let rest = line.strip_prefix(MARK)?.trim_start();
    let mut fields = rest.split(SEP);
    let reason = fields.next()?.trim();
    if reason.is_empty() {
        return None;
    }
    let wait = fields.find_map(|field| {
        RETRY_PHRASES.iter().find_map(|phrase| {
            let after = field.trim().strip_prefix(phrase)?;
            // "5s (resets 3pm)" — the reset time is the reason's business.
            let wait = after.split(" (").next().unwrap_or(after).trim();
            Some(match phrase.contains("up to") {
                true => format!("\u{2264}{wait}"),
                false => wait.to_string(),
            })
        })
    })?;
    Some(Stall {
        reason: reason.to_string(),
        wait: (!wait.is_empty()).then_some(wait),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stall(reason: &str, wait: &str) -> Option<Stall> {
        Some(Stall {
            reason: reason.into(),
            wait: Some(wait.into()),
        })
    }

    #[test]
    fn the_network_stall() {
        assert_eq!(
            detect("✻ Waiting for API response · will retry in 2m 34s · check your network"),
            stall("Waiting for API response", "2m 34s"),
        );
    }

    #[test]
    fn an_error_retry_with_a_reset_time() {
        assert_eq!(
            detect("✻ Usage limit reached · Retrying in 4h 2m (resets 3pm) · attempt 3/10"),
            stall("Usage limit reached", "4h 2m"),
        );
    }

    #[test]
    fn the_line_a_closed_port_draws() {
        // Off the grid of a real Claude Code 2.1.281 with
        // ANTHROPIC_BASE_URL on a refused port: the mark, an empty cell
        // (it jumps to column 3), the reason cut to fit the row.
        assert_eq!(
            detect(
                "✻ Connection refused — a firewall or proxy may be blocking it (EC… · Retrying in 1s · attempt 1/10"
            ),
            stall(
                "Connection refused — a firewall or proxy may be blocking it (EC…",
                "1s"
            ),
        );
    }

    #[test]
    fn no_response_reports_the_bound() {
        assert_eq!(
            detect(
                "✻ No response from the API after 45s · retrying, waiting up to 2m · attempt 1/10"
            ),
            stall("No response from the API after 45s", "≤2m"),
        );
        assert_eq!(
            detect("✻ No response from the API after 3m · retrying once, waiting up to 5m"),
            stall("No response from the API after 3m", "≤5m"),
        );
    }

    #[test]
    fn low_priority_waiting() {
        assert_eq!(
            detect("✻ Server busy · next try in 30s · attempt 3 · esc to interrupt"),
            stall("Server busy", "30s"),
        );
    }

    #[test]
    fn the_working_spinner_is_not_a_stall() {
        assert_eq!(
            detect("✻ Pondering… (12s · ↓ 1.2k tokens · esc to interrupt)"),
            None
        );
        assert_eq!(detect("✻ Cogitating…"), None);
    }

    #[test]
    fn prose_about_the_line_is_not_a_stall() {
        // What an agent prints while discussing this very feature.
        let prose = "\
⏺ The screenshot shows Waiting for API response · will retry in 2m 34s
  ✻ Waiting for API response · will retry in 2m 34s · check your network
  `✻ Waiting for API response · will retry in 2m 34s`";
        assert_eq!(detect(prose), None);
    }

    #[test]
    fn the_newest_line_wins() {
        let rows = "\
✻ Waiting for API response · will retry in 10s · check your network
some output
✻ API Error (Connection error.) · Retrying in 2s · attempt 4/10
╭──────────╮
│ >        │";
        assert_eq!(detect(rows), stall("API Error (Connection error.)", "2s"));
    }
}
