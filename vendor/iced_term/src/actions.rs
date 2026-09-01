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
    #[default]
    Ignore,
}
