#[derive(Debug, Clone, PartialEq, Default)]
pub enum Action {
    Shutdown,
    ChangeTitle(String),
    /// A mouse selection finished — the embedder should offer this text as
    /// the PRIMARY selection (VTE does this internally; iced clipboard tasks
    /// live at the application layer, so it surfaces as an action).
    PublishSelection(String),
    /// A gesture that could have selected finished in a pane whose
    /// APPLICATION owns the mouse (a report drag across cells, or a
    /// double/triple report click). The widget has no text — every event
    /// went to the app — so the embedder decides whether a selection
    /// exists and where to collect it (tmux keeps its mouse copies in
    /// paste buffers reachable over ssh). A plain click never gets here.
    ReportedSelectionGesture,
    /// Outcome of a `SearchNext` command — whether a match is now focused.
    SearchResult(bool),
    /// The user opened a hovered link (Ctrl+click). The embedder launches
    /// the browser — after rewriting the URL if the terminal is remote
    /// (the printed port is the host's; only a tunnel makes it local).
    OpenUrl(String),
    /// Space is auto-repeating in a terminal whose embedder asked for hold
    /// reporting (`terminal::Command::SetHoldRelay`). The repeat was NOT
    /// written to the PTY: the embedder decides how the hold reaches the
    /// application — over a jittery link, by generating the repeats where
    /// the application runs (Claude Code's hold-to-talk ends a recording
    /// at the first 200 ms gap between repeats).
    HoldRepeat,
    /// Space was released in such a terminal. Sent whether or not a hold
    /// was underway; the embedder ignores the ones it wasn't relaying.
    HoldRelease,
    #[default]
    Ignore,
}
