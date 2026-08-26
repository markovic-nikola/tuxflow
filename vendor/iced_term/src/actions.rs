#[derive(Debug, Clone, PartialEq, Default)]
pub enum Action {
    Shutdown,
    ChangeTitle(String),
    /// A mouse selection finished — the embedder should offer this text as
    /// the PRIMARY selection (VTE does this internally; iced clipboard tasks
    /// live at the application layer, so it surfaces as an action).
    PublishSelection(String),
    /// Outcome of a `SearchNext` command — whether a match is now focused.
    SearchResult(bool),
    /// The user opened a hovered link (Ctrl+click). The embedder launches
    /// the browser — after rewriting the URL if the terminal is remote
    /// (the printed port is the host's; only a tunnel makes it local).
    OpenUrl(String),
    #[default]
    Ignore,
}
