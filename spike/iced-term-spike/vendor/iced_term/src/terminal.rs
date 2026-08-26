use crate::actions::Action;
use crate::backend;
use crate::bindings::{Binding, BindingAction, BindingsLayout, InputKind};
use crate::font::TermFont;
use crate::settings::{FontSettings, Settings, ThemeSettings};
use crate::theme::{ColorPalette, Theme};
use crate::AlacrittyEvent;
use iced::futures::stream::BoxStream;
use iced::futures::{SinkExt, StreamExt};
use iced::widget::canvas::Cache;
use iced::Subscription;
use std::hash::{Hash, Hasher};
use std::io::Result;
use std::sync::Arc;
use tokio::sync::mpsc::{self, UnboundedReceiver};
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub enum Event {
    BackendCall(u64, backend::Command),
}

#[derive(Debug, Clone)]
pub enum Command {
    ChangeTheme(Box<ColorPalette>),
    ChangeFont(FontSettings),
    AddBindings(Vec<(Binding<InputKind>, BindingAction)>),
    ProxyToBackend(backend::Command),
}

pub struct Terminal {
    pub id: u64,
    widget_id: iced::widget::Id,
    pub(crate) font: TermFont,
    pub(crate) theme: Theme,
    pub(crate) cache: Cache,
    pub(crate) bindings: BindingsLayout,
    pub(crate) backend: backend::Backend,
    backend_event_rx: Arc<Mutex<UnboundedReceiver<AlacrittyEvent>>>,
}

impl Terminal {
    pub fn new(id: u64, settings: Settings) -> Result<Self> {
        // Unbounded on purpose: Term emits events (MouseCursorDirty,
        // PtyWrite, ...) WHILE the PTY thread holds the terminal lock. A
        // bounded channel deadlocks under output floods: PTY thread blocks
        // sending (lock held) -> UI thread blocks on the lock in sync() ->
        // forwarding task blocks on the full iced channel because the UI
        // thread isn't draining messages. The drain-and-coalesce loop below
        // keeps this queue near-empty in practice.
        let (backend_event_tx, backend_event_rx) = mpsc::unbounded_channel();
        let theme = Theme::new(settings.theme);
        let font = TermFont::new(settings.font);

        Ok(Self {
            id,
            widget_id: iced::widget::Id::unique(),
            font,
            theme,
            bindings: BindingsLayout::default(),
            cache: Cache::default(),
            backend: backend::Backend::new(
                id,
                backend_event_tx,
                settings.backend,
            )?,
            backend_event_rx: Arc::new(Mutex::new(backend_event_rx)),
        })
    }

    pub fn backend(&self) -> &backend::Backend {
        &self.backend
    }

    pub fn widget_id(&self) -> &iced::widget::Id {
        &self.widget_id
    }

    pub fn subscription(&self) -> Subscription<Event> {
        let data = TerminalSubscriptionData {
            id: self.id,
            event_receiver: self.backend_event_rx.clone(),
        };

        Subscription::run_with(data, terminal_subscription_stream)
    }

    pub fn handle(&mut self, cmd: Command) -> Action {
        let mut action = Action::default();

        match cmd {
            Command::ChangeTheme(color_pallete) => {
                self.theme = Theme::new(ThemeSettings::new(color_pallete));
                self.redraw();
            },
            Command::ChangeFont(font_settings) => {
                self.font = TermFont::new(font_settings);
                self.sync_and_redraw();
            },
            Command::AddBindings(bindings) => {
                self.bindings.add_bindings(bindings);
            },
            Command::ProxyToBackend(cmd) => {
                // Snapshotting the viewport and clearing the canvas cache on
                // every event is wasted work for commands that cannot change
                // what is displayed (mouse reports, hover) — classify first.
                let needs_sync = proxied_cmd_changes_content(&cmd);
                let link_redraw =
                    matches!(cmd, backend::Command::ProcessLink(..));
                action = self.backend.handle(cmd);
                // Don't clear the canvas cache for a snapshot that was
                // skipped on lock contention — the next Wakeup retries.
                let synced = needs_sync && self.backend.sync();
                if synced || link_redraw {
                    self.redraw();
                }
            },
        };

        action
    }

    fn sync_and_redraw(&mut self) {
        self.sync_font();
        let _ = self.backend.sync();
        self.redraw();
    }

    fn sync_font(&mut self) {
        self.font.sync();
        self.backend
            .handle(backend::Command::Resize(None, Some(self.font.measure)));
    }

    fn redraw(&mut self) {
        self.cache.clear();
    }
}

fn proxied_cmd_changes_content(cmd: &backend::Command) -> bool {
    match cmd {
        backend::Command::Write(_)
        | backend::Command::Scroll(_)
        | backend::Command::Resize(..)
        | backend::Command::SelectStart(..)
        | backend::Command::SelectUpdate(_)
        // Search scrolls to the match and moves the highlight.
        | backend::Command::SearchNext(..)
        | backend::Command::SearchClear => true,
        backend::Command::ProcessAlacrittyEvent(event) => {
            matches!(event, AlacrittyEvent::Wakeup | AlacrittyEvent::Exit)
        },
        // SelectRelease reads the finished selection; it changes nothing
        // on screen.
        backend::Command::SelectRelease
        | backend::Command::ProcessLink(..)
        | backend::Command::MouseReport(..) => false,
    }
}

#[derive(Clone)]
struct TerminalSubscriptionData {
    id: u64,
    event_receiver: Arc<Mutex<UnboundedReceiver<AlacrittyEvent>>>,
}

impl Hash for TerminalSubscriptionData {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

fn terminal_subscription_stream(
    data: &TerminalSubscriptionData,
) -> BoxStream<'static, Event> {
    let id = data.id;
    let event_receiver = data.event_receiver.clone();
    iced::stream::channel(1000, async move |mut output| {
        let mut shutdown = false;
        loop {
            let mut event_receiver = event_receiver.lock().await;
            match event_receiver.recv().await {
                Some(event) => {
                    // A flooding PTY (`yes`) emits Wakeups far faster than
                    // the app can sync + redraw; forwarding each one starves
                    // the UI thread until the desktop's not-responding
                    // watchdog fires. Drain the burst: forward non-Wakeup
                    // events in order, collapse every Wakeup into a single
                    // trailing one — content syncs at the app's own pace.
                    let mut wakeup = false;
                    let mut mouse_dirty = false;
                    let mut blink_changed = false;
                    let mut events = Vec::new();
                    let mut next = Some(event);
                    loop {
                        let ev = match next.take() {
                            Some(ev) => ev,
                            None => match event_receiver.try_recv() {
                                Ok(ev) => ev,
                                Err(_) => break,
                            },
                        };
                        if matches!(ev, AlacrittyEvent::Exit) {
                            shutdown = true;
                        }
                        if matches!(ev, AlacrittyEvent::Wakeup) {
                            wakeup = true;
                        } else if matches!(ev, AlacrittyEvent::MouseCursorDirty)
                        {
                            // Emitted per scroll during floods — collapse
                            // like Wakeup or it spams the message queue.
                            mouse_dirty = true;
                        } else if matches!(
                            ev,
                            AlacrittyEvent::CursorBlinkingChange
                        ) {
                            blink_changed = true;
                        } else {
                            events.push(ev);
                        }
                        if events.len() >= 512 {
                            break;
                        }
                    }
                    if mouse_dirty {
                        events.push(AlacrittyEvent::MouseCursorDirty);
                    }
                    if blink_changed {
                        events.push(AlacrittyEvent::CursorBlinkingChange);
                    }
                    if wakeup {
                        events.push(AlacrittyEvent::Wakeup);
                    }

                    for ev in events {
                        output
                            .send(Event::BackendCall(
                                id,
                                backend::Command::ProcessAlacrittyEvent(ev),
                            ))
                            .await
                            .unwrap_or_else(|_| {
                                panic!("iced_term stream {}: sending BackendEventReceived event is failed", id)
                            });
                    }
                },
                None => {
                    if !shutdown {
                        panic!("iced_term stream {}: terminal event channel closed unexpected", id);
                    }
                    // Upstream looped forever here: recv() on a closed,
                    // drained channel returns None immediately — a hot spin
                    // burning a core per exited terminal. End the stream.
                    return;
                },
            }
        }
    })
    .boxed()
}
