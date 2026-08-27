//! Is an agent *working*, or just sitting at its prompt?
//!
//! Both shells answer from the same signal — the terminal's
//! "contents changed" notification (VTE's signal in GTK, alacritty's
//! `Wakeup` on iced) — sampled on a fixed interval, and both need the same
//! hysteresis, so the rule lives here rather than in each shell.
//!
//! Turning ON needs a genuine repaint BURST: a working agent redraws its
//! spinner continuously, dozens of events per sample, which keeps the few
//! trailing repaints after it finishes from flapping the indicator. Once
//! on, brief lulls (an agent thinking between tool calls) are ridden out
//! by a recent-activity window instead.

use std::time::Duration;

/// How often callers sample. `events` counts repaints since the last one.
pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(2);
/// Repaints within one sample that read as "it started working".
pub const BURST_MIN: u32 = 3;
/// How long an already-working agent may go quiet before it reads as done.
pub const QUIET_WINDOW: Duration = Duration::from_secs(4);

/// The working flag for the next sample, given the current one.
pub fn next_working(was_working: bool, events: u32, since_activity: Duration) -> bool {
    if was_working {
        since_activity < QUIET_WINDOW
    } else {
        events >= BURST_MIN
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lone_repaint_does_not_start_it() {
        // Echoing a keystroke, or an agent's final line — not work.
        assert!(!next_working(false, 1, Duration::ZERO));
        assert!(!next_working(false, 2, Duration::ZERO));
    }

    #[test]
    fn a_burst_starts_it() {
        assert!(next_working(false, BURST_MIN, Duration::ZERO));
        assert!(next_working(false, 40, Duration::ZERO));
    }

    #[test]
    fn a_lull_keeps_it_on() {
        // Thinking between tool calls: no repaints this sample, still working.
        assert!(next_working(
            true,
            0,
            QUIET_WINDOW - Duration::from_millis(1)
        ));
    }

    #[test]
    fn silence_past_the_window_ends_it() {
        assert!(!next_working(true, 0, QUIET_WINDOW));
        assert!(!next_working(true, 99, QUIET_WINDOW));
    }
}
