#[derive(Debug, Clone, PartialEq, Default)]
pub enum Action {
    Shutdown,
    ChangeTitle(String),
    /// A mouse selection finished — the embedder should offer this text as
    /// the PRIMARY selection (VTE does this internally; iced clipboard tasks
    /// live at the application layer, so it surfaces as an action).
    PublishSelection(String),
    #[default]
    Ignore,
}
