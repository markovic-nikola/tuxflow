//! The Git Changes view — GTK's `git_changes_dialog` as a full-pane view,
//! the same shape `settings_ui` takes. File list on the left, the selected
//! file's diff on the right, a commit box and Push/Pull underneath.
//!
//! Every git call lives in `tuxflow_core::remote::git`; this module owns
//! only the widgets and the state machine around them. Async results
//! arrive as `Msg` variants stamped with the `generation` they were issued
//! under, because a remote project's round trips are seconds long and the
//! user can switch files (or refresh, or pull) while one is in flight —
//! an unstamped reply would paint the previous file's diff over the
//! current one.

use iced::widget::{button, column, container, row, scrollable, text, text_editor};
use iced::{Element, Length};

use tuxflow_core::remote::ProjectLocation;
use tuxflow_core::remote::git::{ChangedFile, DiffResult, FileStatus};

use crate::theme::{
    self, DIM, GIT_ADDED, GIT_BEHIND, GIT_MODIFIED, GIT_REMOVED, GIT_UNTRACKED, TEXT,
    TEXT_SECONDARY, bold,
};

/// A write action in flight. The buttons go quiet while one runs, and the
/// poll leaves the counter it owns alone — a stale ↑2 painted over a push
/// that just finished reads as "the push did nothing".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Busy {
    Commit,
    Push,
    Pull,
}

impl Busy {
    fn label(self) -> &'static str {
        match self {
            Self::Commit => "Committing…",
            Self::Push => "Pushing…",
            Self::Pull => "Pulling…",
        }
    }

    pub fn failure_heading(self) -> &'static str {
        match self {
            Self::Commit => "Commit Failed",
            Self::Push => "Push Failed",
            Self::Pull => "Pull Failed",
        }
    }
}

pub struct State {
    /// Which project this view belongs to. Closing the project, or
    /// switching to another one, closes the view rather than letting it
    /// keep polling a repo nobody is looking at.
    pub project: u64,
    pub location: ProjectLocation,
    pub files: Vec<ChangedFile>,
    pub selected: Option<usize>,
    pub diff: Option<DiffResult>,
    pub diff_loading: bool,
    pub loading: bool,
    pub message: text_editor::Content,
    pub ahead: usize,
    pub behind: usize,
    pub branch: Option<String>,
    pub busy: Option<Busy>,
    /// (heading, body) of the last failed write action — GTK opens an
    /// AlertDialog; here it is a banner the user dismisses.
    pub error: Option<(String, String)>,
    /// Last porcelain hash seen by the poll; a change reloads the list.
    pub last_hash: u64,
    /// Ticks since the view opened — a fetch every 15th (~30 s), as in GTK.
    pub ticks: u32,
    /// Bumped by anything that invalidates in-flight async work.
    pub generation: u64,
    /// Identifies THIS view's poll chain (see [`Msg::Tick`]).
    pub stamp: u64,
}

impl State {
    pub fn new(project: u64, location: ProjectLocation, seed: Seed, stamp: u64) -> Self {
        Self {
            stamp,
            project,
            location,
            files: Vec::new(),
            selected: None,
            diff: None,
            diff_loading: false,
            loading: true,
            message: text_editor::Content::new(),
            ahead: seed.ahead,
            behind: seed.behind,
            branch: seed.branch,
            busy: None,
            error: None,
            last_hash: 0,
            ticks: 0,
            generation: 0,
        }
    }

    pub fn bump(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }

    pub fn commit_message(&self) -> String {
        self.message.text().trim().to_string()
    }

    pub fn selected_file(&self) -> Option<&ChangedFile> {
        self.selected.and_then(|i| self.files.get(i))
    }
}

/// What the status-bar poller already knows, so the view opens with the
/// branch and Push/Pull already right instead of blank for a round trip.
pub struct Seed {
    pub ahead: usize,
    pub behind: usize,
    pub branch: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Close,
    Refresh,
    SelectFile(usize),
    MessageAction(text_editor::Action),
    Commit,
    Push,
    Pull,
    DismissError,
    /// Self-chaining 2 s poll, stamped so a close-then-reopen doesn't
    /// leave two chains running against one view.
    Tick(u64),
    // ── async arrivals, all generation-stamped ──────────────────────────
    Files {
        generation: u64,
        files: Vec<ChangedFile>,
    },
    Diff {
        generation: u64,
        /// Which file this diff is OF. The generation alone can't tell two
        /// diffs apart when both were requested under it — two quick file
        /// clicks race, and on a remote project the first (slow) reply
        /// would paint over the second file's diff.
        path: String,
        diff: Box<DiffResult>,
    },
    Sync {
        generation: u64,
        ahead: usize,
        behind: usize,
        branch: Option<String>,
        hash: u64,
    },
    Done {
        /// Deliberately NOT generation-stamped: only one write action can
        /// be in flight (`git_run` refuses while busy), and `busy` must be
        /// cleared by exactly the action that set it — a generation gate
        /// here let any list reload orphan the flag, wedging the view in
        /// "Pushing…" with every button disabled.
        action: Busy,
        result: Result<(), String>,
    },
}

fn status_color(status: FileStatus) -> iced::Color {
    match status {
        FileStatus::Modified | FileStatus::Renamed => GIT_MODIFIED,
        FileStatus::Added => GIT_ADDED,
        FileStatus::Deleted => GIT_REMOVED,
        FileStatus::Untracked => GIT_UNTRACKED,
    }
}

pub fn view(state: &'_ State) -> Element<'_, Msg> {
    let header = row![
        button(text("Close").size(12))
            .padding([5, 14])
            .style(theme::pill_button(TEXT_SECONDARY))
            .on_press(Msg::Close),
        text(match &state.branch {
            Some(b) => format!("\u{2387} {b}"),
            None => String::from("Git Changes"),
        })
        .size(13)
        .font(bold()),
        iced::widget::space::horizontal(),
        button(text("Refresh").size(12))
            .padding([5, 14])
            .style(theme::pill_button(TEXT_SECONDARY))
            .on_press(Msg::Refresh),
    ]
    .spacing(10)
    .align_y(iced::Alignment::Center);

    let body: Element<'_, Msg> = if state.loading {
        centered("Loading\u{2026}")
    } else if state.files.is_empty() {
        centered("No changes")
    } else {
        row![
            container(scrollable(file_list(state)).style(theme::overlay_scrollbar))
                .width(260)
                .height(Length::Fill),
            container(column![])
                .width(1)
                .height(Length::Fill)
                .style(theme::hairline),
            container(diff_pane(state))
                .width(Length::Fill)
                .height(Length::Fill),
        ]
        .into()
    };

    let mut root = column![header].spacing(10).padding(12);
    if let Some((heading, detail)) = &state.error {
        root = root.push(error_banner(heading, detail));
    }
    root = root
        .push(
            container(body)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(theme::settings_card),
        )
        .push(commit_bar(state));

    container(root)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(theme::ground)
        .into()
}

fn centered(label: &str) -> Element<'_, Msg> {
    container(text(label).size(15).color(DIM))
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
}

fn error_banner<'a>(heading: &'a str, detail: &'a str) -> Element<'a, Msg> {
    container(
        row![
            column![
                text(heading).size(12).font(bold()).color(GIT_REMOVED),
                text(detail).size(11).color(TEXT_SECONDARY),
            ]
            .spacing(2)
            .width(Length::Fill),
            button(text("\u{2715}").size(12))
                .padding([2, 6])
                .style(theme::ghost(TEXT))
                .on_press(Msg::DismissError),
        ]
        .align_y(iced::Alignment::Center),
    )
    .padding([8, 12])
    .width(Length::Fill)
    .style(theme::pill)
    .into()
}

fn file_list(state: &'_ State) -> Element<'_, Msg> {
    let mut list = column![].spacing(1).padding(4);
    for (index, file) in state.files.iter().enumerate() {
        let selected = state.selected == Some(index);
        list = list.push(
            button(
                row![
                    text(file.status.label())
                        .size(11)
                        .font(bold())
                        .color(status_color(file.status))
                        .width(16),
                    // Long paths are elided from the START: the tail
                    // (the filename) is what tells two of them apart.
                    text(elide_start(&file.path, 34)).size(12).color(TEXT),
                ]
                .spacing(6)
                .align_y(iced::Alignment::Center),
            )
            .padding([5, 8])
            .width(Length::Fill)
            .style(theme::process_row(theme::LOCAL_ACCENT, selected))
            .on_press(Msg::SelectFile(index)),
        );
    }
    list.into()
}

/// Keep the tail, drop the head — `…/src/ui/window.rs` beats
/// `src/ui/very/long/pa…`, since the filename is the identifying part.
fn elide_start(path: &str, max_chars: usize) -> String {
    let count = path.chars().count();
    if count <= max_chars {
        return path.to_string();
    }
    let skip = count - max_chars + 1;
    format!("\u{2026}{}", path.chars().skip(skip).collect::<String>())
}

/// The diff pane's type. The line height is pinned rather than left to the
/// default because the marker column and the code column are two separate
/// widgets — they only stay on the same row if they agree on how tall a
/// row is.
const DIFF_SIZE: f32 = 12.0;
const DIFF_LINE_HEIGHT: f32 = 1.5;
/// The marker gutter. Fixed width, so `+` and `\u{2212}` line up in a column
/// of their own instead of shifting the code by their own advance.
const MARKER_W: f32 = 16.0;
/// The removed side is dimmed so the eye lands on the code that survives.
const DEL_ALPHA: f32 = 0.62;

#[derive(Clone, Copy, PartialEq, Eq)]
enum LineKind {
    Add,
    Del,
    Ctx,
}

impl LineKind {
    fn of(line: &str) -> Self {
        match line.as_bytes().first() {
            Some(b'+') if !line.starts_with("+++") => Self::Add,
            Some(b'-') if !line.starts_with("---") => Self::Del,
            _ => Self::Ctx,
        }
    }

    fn marker(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Del => "\u{2212}",
            Self::Ctx => " ",
        }
    }

    /// Where the code starts, now that the marker is drawn in its own
    /// column and must not be repeated in the text. Context lines carry a
    /// leading space, but `\ No newline at end of file` does not — and
    /// slicing a byte off that one would eat its backslash.
    fn code_start(self, line: &str) -> usize {
        match self {
            Self::Add | Self::Del => 1,
            Self::Ctx if line.starts_with(' ') => 1,
            Self::Ctx => 0,
        }
    }
}

fn diff_pane(state: &'_ State) -> Element<'_, Msg> {
    if state.diff_loading {
        return centered("Loading\u{2026}");
    }
    let Some(diff) = &state.diff else {
        return centered("Select a file");
    };
    if diff.text.is_empty() {
        return centered("(no diff available)");
    }

    // Both lists arrive sorted by line, so one forward cursor each hands
    // every line its own slice without allocating or re-scanning.
    let (marks, words) = (&diff.highlights, &diff.word_ranges);
    let (mut mi, mut wi) = (0usize, 0usize);
    let mut body = column![].width(Length::Fill);

    for (index, line) in diff.text.lines().enumerate() {
        while mi < marks.len() && marks[mi].0 < index {
            mi += 1;
        }
        let m0 = mi;
        while mi < marks.len() && marks[mi].0 == index {
            mi += 1;
        }
        while wi < words.len() && words[wi].0 < index {
            wi += 1;
        }
        let w0 = wi;
        while wi < words.len() && words[wi].0 == index {
            wi += 1;
        }
        body = body.push(diff_line(line, &marks[m0..mi], &words[w0..wi]));
    }

    // Vertical only. `Length::Fill` inside a horizontally-scrollable
    // resolves against the VIEWPORT, so a full-width band would stop at
    // the right edge and scrolling would reveal untinted line past it —
    // long lines wrap instead.
    scrollable(container(body).padding([8, 0]).width(Length::Fill))
        .style(theme::overlay_scrollbar)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// One diff line: marker in its own column, code beside it, the whole row
/// wearing the band so the tint reaches both edges of the pane.
fn diff_line<'a>(
    line: &'a str,
    marks: &[(usize, usize, usize, String)],
    words: &[(usize, usize, usize)],
) -> Element<'a, Msg> {
    let kind = LineKind::of(line);
    let body = row![
        text(kind.marker())
            .size(DIFF_SIZE)
            .line_height(iced::widget::text::LineHeight::Relative(DIFF_LINE_HEIGHT))
            .align_x(iced::widget::text::Alignment::Center)
            .width(MARKER_W)
            .color(match kind {
                LineKind::Add => theme::alpha(GIT_ADDED, 0.9),
                LineKind::Del => theme::alpha(GIT_REMOVED, 0.75),
                LineKind::Ctx => DIM,
            }),
        iced::widget::rich_text(line_spans(line, kind, marks, words))
            .size(DIFF_SIZE)
            .line_height(iced::widget::text::LineHeight::Relative(DIFF_LINE_HEIGHT))
            .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
            .width(Length::Fill),
    ]
    .spacing(6)
    .align_y(iced::Alignment::Start);

    let cell = container(body).width(Length::Fill).padding([0, 8]);
    match kind {
        LineKind::Add => cell.style(theme::diff_band(GIT_ADDED)),
        LineKind::Del => cell.style(theme::diff_band(GIT_REMOVED)),
        LineKind::Ctx => cell,
    }
    .into()
}

/// Turn one line's syntax colours and word-diff ranges into text spans.
///
/// Two independent range lists have to land on the same string: syntect's
/// `(offset, len, hex)` tokens decide COLOUR, core's word ranges decide
/// which stretches get the emphasis BACKGROUND. Neither is a subdivision
/// of the other, so the syntax walk drives and each of its pieces is then
/// re-cut by any word range crossing it.
///
/// The band itself is no longer here — it moved to the row's container so
/// it spans the full pane width.
fn line_spans<'a>(
    line: &'a str,
    kind: LineKind,
    marks: &[(usize, usize, usize, String)],
    words: &[(usize, usize, usize)],
) -> Vec<iced::widget::text::Span<'a, ()>> {
    let dim = kind == LineKind::Del;
    let emphasis = match kind {
        LineKind::Add => Some(theme::alpha(GIT_ADDED, 0.34)),
        LineKind::Del => Some(theme::alpha(GIT_REMOVED, 0.30)),
        LineKind::Ctx => None,
    };

    let mut spans = Vec::new();
    let mut cursor = kind.code_start(line);

    for (_, offset, len, hex) in marks {
        // syntect hands back token boundaries, but the line is sliced by
        // BYTE offset — a highlight that somehow straddled a char boundary
        // would panic, so bail on anything unslicable.
        let (offset, end) = (*offset, offset + len);
        if offset < cursor
            || end > line.len()
            || !line.is_char_boundary(offset)
            || !line.is_char_boundary(end)
        {
            continue;
        }
        if offset > cursor {
            push_run(
                &mut spans,
                line,
                cursor,
                offset,
                tint(TEXT_SECONDARY, dim),
                words,
                emphasis,
            );
        }
        push_run(
            &mut spans,
            line,
            offset,
            end,
            tint(theme::hex(hex), dim),
            words,
            emphasis,
        );
        cursor = end;
    }
    if cursor < line.len() {
        let plain = tint(TEXT_SECONDARY, dim);
        push_run(&mut spans, line, cursor, line.len(), plain, words, emphasis);
    }

    // A blank line has nothing to shape, and a rich_text with no spans
    // collapses to zero height — which would drop the row out of the
    // pane's rhythm and take its band with it.
    if spans.is_empty() {
        spans.push(iced::widget::span(" "));
    }
    spans
}

/// Emit `line[from..to]` in one colour, split wherever a word-diff range
/// crosses it so the changed stretches can carry the emphasis background.
fn push_run<'a>(
    out: &mut Vec<iced::widget::text::Span<'a, ()>>,
    line: &'a str,
    from: usize,
    to: usize,
    color: iced::Color,
    words: &[(usize, usize, usize)],
    emphasis: Option<iced::Color>,
) {
    use iced::widget::span;

    let mut at = from;
    for (_, offset, len) in words {
        let (start, end) = ((*offset).max(at), (offset + len).min(to));
        if start >= end || !line.is_char_boundary(start) || !line.is_char_boundary(end) {
            continue;
        }
        if start > at {
            out.push(span(&line[at..start]).color(color));
        }
        let marked = span(&line[start..end]).color(color);
        out.push(match emphasis {
            Some(bg) => marked.background(bg),
            None => marked,
        });
        at = end;
    }
    if at < to {
        out.push(span(&line[at..to]).color(color));
    }
}

/// Fade the removed side. Alpha rather than a blend toward the ground:
/// the band sits behind it, so a blend would have to know the band's
/// colour to stay honest.
fn tint(color: iced::Color, dim: bool) -> iced::Color {
    if dim {
        iced::Color {
            a: DEL_ALPHA,
            ..color
        }
    } else {
        color
    }
}

fn commit_bar(state: &'_ State) -> Element<'_, Msg> {
    let idle = state.busy.is_none();
    let has_message = !state.commit_message().is_empty();

    let mut commit = button(
        text(match state.busy {
            Some(Busy::Commit) => Busy::Commit.label(),
            _ => "Commit",
        })
        .size(12),
    )
    .padding([6, 16])
    .style(theme::primary(theme::LOCAL_ACCENT));
    if idle && has_message {
        commit = commit.on_press(Msg::Commit);
    }

    let mut actions = row![
        commit,
        text(match &state.branch {
            Some(b) => format!("\u{2387} {b}"),
            None => String::new(),
        })
        .size(11)
        .color(DIM),
        iced::widget::space::horizontal(),
    ]
    .spacing(10)
    .align_y(iced::Alignment::Center);

    // Pull only appears when there is something to pull — GTK hides it
    // outright rather than showing a dead button.
    if state.behind > 0 || state.busy == Some(Busy::Pull) {
        let mut pull = button(
            text(match state.busy {
                Some(Busy::Pull) => Busy::Pull.label().to_string(),
                _ => format!("Pull ({})", state.behind),
            })
            .size(12),
        )
        .padding([6, 16])
        .style(theme::pill_button(GIT_BEHIND));
        if idle {
            pull = pull.on_press(Msg::Pull);
        }
        actions = actions.push(pull);
    }

    let mut push = button(
        text(match state.busy {
            Some(Busy::Push) => Busy::Push.label().to_string(),
            _ if state.ahead > 0 => format!("Push ({})", state.ahead),
            _ => String::from("Push"),
        })
        .size(12),
    )
    .padding([6, 16])
    .style(theme::pill_button(GIT_ADDED));
    if idle && state.ahead > 0 {
        push = push.on_press(Msg::Push);
    }
    actions = actions.push(push);

    column![
        container(
            text_editor(&state.message)
                .placeholder("Commit message\u{2026}")
                .height(72)
                .padding(8)
                .style(theme::editor(theme::LOCAL_ACCENT))
                .on_action(Msg::MessageAction),
        )
        .width(Length::Fill),
        actions,
    ]
    .spacing(8)
    .into()
}

/// Compact count for the status-bar chip: 931, 1.2K, 45K. Exact numbers
/// stay in the tooltip. Shared with GTK's `StatusBar::compact_count`
/// wording by having the same breakpoints.
pub fn compact_count(n: usize) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=9_999 => {
            let k = (n as f64 / 100.0).round() / 10.0;
            if k.fract() == 0.0 {
                format!("{}K", k as usize)
            } else {
                format!("{k:.1}K")
            }
        }
        _ => format!("{}K", (n as f64 / 1000.0).round() as usize),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_counts_match_gtk_breakpoints() {
        assert_eq!(compact_count(0), "0");
        assert_eq!(compact_count(931), "931");
        assert_eq!(compact_count(1_000), "1K");
        assert_eq!(compact_count(1_240), "1.2K");
        assert_eq!(compact_count(9_999), "10K");
        assert_eq!(compact_count(45_400), "45K");
    }

    #[test]
    fn elides_from_the_start_keeping_the_filename() {
        assert_eq!(elide_start("src/main.rs", 34), "src/main.rs");
        let long = "tuxflow-iced/src/very/deep/path/to/window.rs";
        let out = elide_start(long, 20);
        assert!(out.starts_with('\u{2026}'), "{out}");
        assert!(out.ends_with("window.rs"), "{out}");
        assert_eq!(out.chars().count(), 20);
    }

    fn rebuild(spans: &[iced::widget::text::Span<'_, ()>]) -> String {
        spans.iter().map(|s| s.text.to_string()).collect()
    }

    /// The walker must emit every byte of the code exactly once — the text
    /// between two syntax tokens included — and must NOT re-emit the
    /// marker, which is drawn in its own column now.
    #[test]
    fn spans_cover_the_code_without_the_marker() {
        let marks = vec![
            (0, 1, 3, String::from("#ff0000")),
            (0, 5, 1, String::from("#00ff00")),
        ];
        let line = "+let x = 1;";
        let spans = line_spans(line, LineKind::Add, &marks, &[]);
        assert_eq!(rebuild(&spans), "let x = 1;");

        // Context lines drop their leading space the same way.
        assert_eq!(
            rebuild(&line_spans(" unchanged", LineKind::Ctx, &[], &[])),
            "unchanged"
        );
    }

    /// `\ No newline at end of file` is a context line that carries no
    /// leading space; slicing one off would eat its backslash.
    #[test]
    fn keeps_the_first_byte_of_an_unprefixed_context_line() {
        let line = "\\ No newline at end of file";
        assert_eq!(rebuild(&line_spans(line, LineKind::Ctx, &[], &[])), line);
    }

    /// A highlight pointing past the end of its line (a desync between the
    /// text and the tuples) must be skipped, not panic on the slice.
    #[test]
    fn spans_survive_out_of_range_highlights() {
        let marks = vec![(0, 1, 99, String::from("#ff0000"))];
        assert_eq!(
            rebuild(&line_spans("+ab", LineKind::Add, &marks, &[])),
            "ab"
        );
    }

    /// A word range must split the run it crosses into exactly the marked
    /// stretch and its neighbours, and only the marked one carries a
    /// background.
    #[test]
    fn word_ranges_get_the_emphasis_background() {
        // "+let x = amount;" — mark "amount" at byte 9, length 6.
        let line = "+let x = amount;";
        let spans = line_spans(line, LineKind::Add, &[], &[(0, 9, 6)]);
        assert_eq!(rebuild(&spans), "let x = amount;");
        let marked: Vec<_> = spans
            .iter()
            .filter(|s| s.highlight.is_some())
            .map(|s| s.text.to_string())
            .collect();
        assert_eq!(marked, vec!["amount"]);
    }

    /// A word range crossing a syntax-token boundary must survive the cut
    /// on both sides — this is the case the two range lists make possible.
    #[test]
    fn word_ranges_survive_crossing_a_syntax_token() {
        let line = "+ab cd";
        let marks = vec![
            (0, 1, 2, String::from("#ff0000")),
            (0, 4, 2, String::from("#00ff00")),
        ];
        // Mark "b cd" — starts inside the first token, ends at the second.
        let spans = line_spans(line, LineKind::Add, &marks, &[(0, 2, 4)]);
        assert_eq!(rebuild(&spans), "ab cd");
        let marked: String = spans
            .iter()
            .filter(|s| s.highlight.is_some())
            .map(|s| s.text.to_string())
            .collect();
        assert_eq!(marked, "b cd");
    }

    /// The removed side is faded; the added side is not.
    #[test]
    fn only_the_removed_side_is_dimmed() {
        let del = line_spans("-gone", LineKind::Del, &[], &[]);
        let add = line_spans("+kept", LineKind::Add, &[], &[]);
        assert!(
            del.iter()
                .all(|s| s.color.is_some_and(|c| c.a == DEL_ALPHA))
        );
        assert!(add.iter().all(|s| s.color.is_some_and(|c| c.a == 1.0)));
    }

    /// An empty line still has to produce a span, or its row collapses to
    /// zero height and takes the band with it.
    #[test]
    fn blank_lines_still_emit_a_span() {
        for line in ["", " ", "+"] {
            let spans = line_spans(line, LineKind::of(line), &[], &[]);
            assert!(!spans.is_empty(), "no spans for {line:?}");
        }
    }
}
