use crate::backend::{
    Backend, Command, LinkAction, MouseButton, RenderableContent,
};
use crate::bindings::{BindingAction, BindingsLayout, InputKind};
use crate::terminal::{Event, Terminal};
use crate::theme::TerminalStyle;
use alacritty_terminal::index::Point as TerminalGridPoint;
use alacritty_terminal::selection::SelectionType;
use alacritty_terminal::term::{cell, TermMode};
use alacritty_terminal::vte::ansi::{self as ansi, NamedColor};
use iced::alignment::Vertical;
use iced::font::{Style as FontStyle, Weight as FontWeight};
use iced::mouse::{Cursor, ScrollDelta};
use iced::widget::canvas::{Path, Text};
use iced::widget::container;
use iced::{Color, Element, Length, Point, Rectangle, Size, Theme};
use iced_core::clipboard::Kind as ClipboardKind;
use iced_core::keyboard::{Key, Modifiers};
use iced_core::mouse::{self, Click};
use iced_core::text::{Alignment, LineHeight, Shaping};
use iced_core::widget::operation::{self, Focusable};
use iced_graphics::core::widget::{tree, Tree};
use iced_graphics::core::Widget;
use iced_graphics::geometry::Stroke;

pub struct TerminalView<'a> {
    term: &'a Terminal,
}

impl<'a> TerminalView<'a> {
    pub fn show(term: &'a Terminal) -> Element<'a, Event> {
        container(Self { term })
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| term.theme.container_style())
            .into()
    }

    pub fn focus<Message: 'static>(
        id: iced::widget::Id,
    ) -> iced::Task<Message> {
        iced::widget::operation::focus(id)
    }

    /// Drop keyboard focus from every focusable — terminals included. For
    /// an embedder raising a MODAL layer (context menu, confirmation card):
    /// stack layers above the base don't capture keyboard events, so a
    /// still-focused terminal underneath keeps eating the keys the layer is
    /// being asked to answer — Esc meant for the menu reaches a running
    /// agent as "interrupt", Enter meant for the dialog hits the shell.
    ///
    /// Implemented as a focus of an id that exists nowhere: the runtime
    /// module wraps `focusable::focus` into a Task but not `unfocus`, and
    /// `focus(target)`'s own contract (focusable.rs) is "focus the match,
    /// unfocus everything else" — with no match it IS unfocus-all.
    pub fn unfocus<Message: 'static>() -> iced::Task<Message> {
        iced::widget::operation::focus(iced::widget::Id::unique())
    }

    fn is_cursor_in_layout(
        &self,
        cursor: Cursor,
        layout: iced_graphics::core::Layout<'_>,
    ) -> bool {
        if let Some(cursor_position) = cursor.position() {
            let layout_position = layout.position();
            let layout_size = layout.bounds();
            let is_triggered = cursor_position.x >= layout_position.x
                && cursor_position.y >= layout_position.y
                && cursor_position.x < (layout_position.x + layout_size.width)
                && cursor_position.y < (layout_position.y + layout_size.height);

            return is_triggered;
        }

        false
    }

    fn is_cursor_hovered_hyperlink(&self, state: &TerminalViewState) -> bool {
        let content = self.term.backend.renderable_content();
        if let Some(hyperlink_range) = &content.hovered_hyperlink {
            return hyperlink_range.contains(&state.mouse_position_on_grid);
        }

        false
    }

    fn handle_resize(
        &mut self,
        state: &mut TerminalViewState,
        layout: iced_graphics::core::Layout<'_>,
        shell: &mut iced_graphics::core::Shell<'_, Event>,
    ) {
        // iced reuses widget state by TYPE at a tree position — showing a
        // DIFFERENT terminal in the same slot (a process switcher) inherits
        // the previous terminal's recorded size, and "size unchanged" would
        // silently skip the new terminal's resize forever. Track which
        // terminal the size was sent to, not just the size.
        let layout_size = layout.bounds().size();
        if state.size != layout_size || state.sized_for != Some(self.term.id) {
            state.size = layout_size;
            state.sized_for = Some(self.term.id);
            let cmd = Command::Resize(
                Some(layout_size),
                Some(self.term.font.measure),
            );
            shell.publish(Event::BackendCall(self.term.id, cmd));
        }
    }

    fn handle_focus(
        &self,
        event: &iced_core::Event,
        state: &mut TerminalViewState,
        is_cursor_in_layout: bool,
    ) {
        use iced::Event::Mouse;
        use iced_core::mouse::{Button::Left, Event::ButtonPressed};

        if let Mouse(ButtonPressed(Left)) = event {
            state.focus = is_cursor_in_layout;
        }
    }

    fn handle_mouse_event(
        &self,
        state: &mut TerminalViewState,
        clipboard: &mut dyn iced_graphics::core::Clipboard,
        layout_position: Point,
        cursor_position: Point,
        event: &iced::mouse::Event,
    ) -> Vec<Command> {
        let mut commands = Vec::new();
        let terminal_content = self.term.backend.renderable_content();
        let terminal_mode = terminal_content.terminal_mode;

        match event {
            iced_core::mouse::Event::ButtonPressed(
                iced_core::mouse::Button::Left,
            ) => {
                if !state.is_focused() {
                    return Vec::default();
                }

                Self::handle_left_button_pressed(
                    state,
                    &terminal_mode,
                    cursor_position,
                    layout_position,
                    &mut commands,
                );
            },
            iced_core::mouse::Event::CursorMoved { position } => {
                if !state.is_focused() {
                    return Vec::default();
                }

                Self::handle_cursor_moved(
                    state,
                    self.term.backend.renderable_content(),
                    position,
                    layout_position,
                    &mut commands,
                );
            },
            iced_core::mouse::Event::ButtonReleased(
                iced_core::mouse::Button::Left,
            ) => {
                if !state.is_focused() {
                    return Vec::default();
                }

                Self::handle_button_released(
                    state,
                    &terminal_mode,
                    &self.term.bindings,
                    &mut commands,
                );
            },
            iced_core::mouse::Event::ButtonPressed(
                iced_core::mouse::Button::Middle,
            ) => {
                if !state.is_focused() {
                    return Vec::default();
                }

                // Middle-click: report to the app when it owns the mouse
                // (Shift bypasses, as everywhere), otherwise paste PRIMARY —
                // the half of the X11 selection convention VTE gives for
                // free.
                if terminal_mode.intersects(TermMode::MOUSE_MODE)
                    && !state.keyboard_modifiers.contains(Modifiers::SHIFT)
                {
                    commands.push(Command::MouseReport(
                        MouseButton::MiddleButton,
                        state.keyboard_modifiers,
                        state.mouse_position_on_grid,
                        true,
                    ));
                } else if let Some(data) =
                    clipboard.read(ClipboardKind::Primary)
                {
                    // Empty = X11 conversion refusal (see the keyboard
                    // Paste arm) — nothing to write.
                    if !data.is_empty() {
                        commands.push(Command::Write(paste_content(
                            &terminal_mode,
                            &data,
                        )));
                    }
                }
            },
            iced_core::mouse::Event::ButtonReleased(
                iced_core::mouse::Button::Middle,
            ) => {
                if state.is_focused()
                    && terminal_mode.intersects(TermMode::MOUSE_MODE)
                    && !state.keyboard_modifiers.contains(Modifiers::SHIFT)
                {
                    commands.push(Command::MouseReport(
                        MouseButton::MiddleButton,
                        state.keyboard_modifiers,
                        state.mouse_position_on_grid,
                        false,
                    ));
                }
            },
            iced::mouse::Event::WheelScrolled { delta } => {
                Self::handle_wheel_scrolled(
                    state,
                    *delta,
                    &terminal_mode,
                    &self.term.font.measure,
                    &mut commands,
                );
            },
            _ => {},
        }

        commands
    }

    fn handle_left_button_pressed(
        state: &mut TerminalViewState,
        terminal_mode: &TermMode,
        cursor_position: Point,
        layout_position: Point,
        commands: &mut Vec<Command>,
    ) {
        // Shift bypasses mouse reporting (the universal terminal
        // convention), so a selection stays reachable when tmux owns the
        // mouse.
        let is_mouse_report = terminal_mode.intersects(TermMode::MOUSE_MODE)
            && !state.keyboard_modifiers.contains(Modifiers::SHIFT);
        let cmd = if is_mouse_report {
            Command::MouseReport(
                MouseButton::LeftButton,
                state.keyboard_modifiers,
                state.mouse_position_on_grid,
                true,
            )
        } else {
            let current_click = Click::new(
                cursor_position,
                mouse::Button::Left,
                state.last_click,
            );
            let selection_type = match current_click.kind() {
                mouse::click::Kind::Single => SelectionType::Simple,
                mouse::click::Kind::Double => SelectionType::Semantic,
                mouse::click::Kind::Triple => SelectionType::Lines,
            };
            state.last_click = Some(current_click);
            Command::SelectStart(
                selection_type,
                (
                    cursor_position.x - layout_position.x,
                    cursor_position.y - layout_position.y,
                ),
            )
        };
        commands.push(cmd);
        state.is_dragged = true;
        state.drag_is_mouse_report = is_mouse_report;
    }

    fn handle_cursor_moved(
        state: &mut TerminalViewState,
        terminal_content: &RenderableContent,
        position: &Point,
        layout_position: Point,
        commands: &mut Vec<Command>,
    ) {
        let cursor_x = position.x - layout_position.x;
        let cursor_y = position.y - layout_position.y;
        state.mouse_position_on_grid = Backend::selection_point(
            cursor_x,
            cursor_y,
            &terminal_content.terminal_size,
            terminal_content.display_offset,
        );

        // Route an active drag the way it started: a report-drag keeps
        // reporting (mode 1002 covers motion-while-pressed, not only 1003),
        // a selection-drag keeps selecting.
        if state.is_dragged {
            let terminal_mode = terminal_content.terminal_mode;
            let cmd = if state.drag_is_mouse_report {
                terminal_mode
                    .intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION)
                    .then(|| {
                        Command::MouseReport(
                            MouseButton::LeftMove,
                            state.keyboard_modifiers,
                            state.mouse_position_on_grid,
                            true,
                        )
                    })
            } else {
                Some(Command::SelectUpdate((cursor_x, cursor_y)))
            };
            if let Some(cmd) = cmd {
                commands.push(cmd);
            }
        }

        // Handle link hover if applicable
        if state.keyboard_modifiers == Modifiers::COMMAND {
            commands.push(Command::ProcessLink(
                LinkAction::Hover,
                state.mouse_position_on_grid,
            ));
        }
    }

    fn handle_button_released(
        state: &mut TerminalViewState,
        terminal_mode: &TermMode,
        bindings: &BindingsLayout, // Use the actual type of your bindings here
        commands: &mut Vec<Command>,
    ) {
        let ended_selection_gesture =
            state.is_dragged && !state.drag_is_mouse_report;
        state.is_dragged = false;

        if state.drag_is_mouse_report
            && terminal_mode.intersects(TermMode::MOUSE_MODE)
        {
            commands.push(Command::MouseReport(
                MouseButton::LeftButton,
                state.keyboard_modifiers,
                state.mouse_position_on_grid,
                false,
            ));
        }
        state.drag_is_mouse_report = false;

        // Every finished selection gesture (drag, double/triple click)
        // offers its text to PRIMARY. An empty selection — a plain click —
        // extracts no text and publishes nothing.
        if ended_selection_gesture {
            commands.push(Command::SelectRelease);
        }

        if bindings.get_action(
            InputKind::Mouse(iced_core::mouse::Button::Left),
            state.keyboard_modifiers,
            *terminal_mode,
        ) == BindingAction::LinkOpen
        {
            commands.push(Command::ProcessLink(
                LinkAction::Open,
                state.mouse_position_on_grid,
            ));
        }
    }

    fn handle_wheel_scrolled(
        state: &mut TerminalViewState,
        delta: ScrollDelta,
        terminal_mode: &TermMode,
        font_measure: &Size<f32>,
        commands: &mut Vec<Command>,
    ) {
        let lines = match delta {
            ScrollDelta::Lines { y, .. } => {
                (y.signum() * y.abs().round()) as i32
            },
            ScrollDelta::Pixels { y, .. } => {
                state.scroll_pixels -= y;
                let line_height = font_measure.height;
                let lines = (state.scroll_pixels / line_height).trunc();
                state.scroll_pixels %= line_height;
                lines as i32
            },
        };

        if lines == 0 {
            return;
        }

        // When the app owns the mouse (tmux), the wheel must arrive as
        // wheel REPORTS (buttons 64/65). Falling through to Scroll turns
        // the wheel into arrow keys at a shell prompt — history browsing
        // instead of tmux copy-mode scrolling. Shift keeps the widget's
        // own scrollback reachable.
        if terminal_mode.intersects(TermMode::MOUSE_MODE)
            && !state.keyboard_modifiers.contains(Modifiers::SHIFT)
        {
            let button = if lines > 0 {
                MouseButton::ScrollUp
            } else {
                MouseButton::ScrollDown
            };
            for _ in 0..lines.abs() {
                commands.push(Command::MouseReport(
                    button.clone(),
                    state.keyboard_modifiers,
                    state.mouse_position_on_grid,
                    true,
                ));
            }
        } else {
            commands.push(Command::Scroll(lines));
        }
    }

    fn handle_keyboard_event(
        &self,
        state: &mut TerminalViewState,
        clipboard: &mut dyn iced_graphics::core::Clipboard,
        event: &iced::keyboard::Event,
    ) -> Option<Command> {
        let mut binding_action = BindingAction::Ignore;
        let last_content = self.term.backend.renderable_content();
        match event {
            iced::keyboard::Event::ModifiersChanged(m) => {
                state.keyboard_modifiers = *m;
                let action = if state.keyboard_modifiers == Modifiers::COMMAND {
                    LinkAction::Hover
                } else {
                    LinkAction::Clear
                };
                return Some(Command::ProcessLink(
                    action,
                    state.mouse_position_on_grid,
                ));
            },
            iced::keyboard::Event::KeyPressed {
                key,
                modifiers,
                text,
                ..
            } => match &key {
                // Use the physical character key for bindings even when text is None (e.g., Ctrl/Cmd combos)
                Key::Character(k) => {
                    let lower = k.to_ascii_lowercase();
                    binding_action = self.term.bindings.get_action(
                        InputKind::Char(lower),
                        state.keyboard_modifiers,
                        last_content.terminal_mode,
                    );

                    // If no binding matched, only write printable text (when provided)
                    if binding_action == BindingAction::Ignore {
                        if let Some(bytes) = text_fallback(
                            text.as_deref(),
                            state.keyboard_modifiers.alt(),
                        ) {
                            return Some(Command::Write(bytes));
                        }
                    }
                },
                Key::Named(code) => {
                    binding_action = self.term.bindings.get_action(
                        InputKind::KeyCode(*code),
                        *modifiers,
                        last_content.terminal_mode,
                    );

                    // Named keys need the SAME no-binding text fallback as
                    // characters, because binding lookup is an EXACT
                    // modifier match: Space is a named key with one bare
                    // row, so a space struck while Shift is still held —
                    // fast prose typing, a capital then the spacebar —
                    // matched nothing and was silently swallowed. The
                    // table keeps priority, so combos with real encodings
                    // (Ctrl+Space = NUL, Shift+Enter, Alt+Enter) are
                    // untouched; keys with no text (F-keys, arrows) still
                    // write nothing.
                    if binding_action == BindingAction::Ignore {
                        if let Some(bytes) = text_fallback(
                            text.as_deref(),
                            state.keyboard_modifiers.alt(),
                        ) {
                            return Some(Command::Write(bytes));
                        }
                    }
                },
                _ => {},
            },
            _ => {},
        }

        match binding_action {
            BindingAction::Char(c) => {
                let mut buf = [0, 0, 0, 0];
                let str = c.encode_utf8(&mut buf);
                return Some(Command::Write(str.as_bytes().to_vec()));
            },
            BindingAction::Esc(seq) => {
                return Some(Command::Write(seq.as_bytes().to_vec()));
            },
            BindingAction::Paste => {
                // Empty is NOT "paste nothing": X11's conversion refusal
                // (an image-only clipboard owner asked for UTF8_STRING)
                // reaches iced as Ok("") — clipboard_x11 maps the
                // SelectionNotify property=None reply to an empty buffer.
                // Writing it would capture the event; returning None lets
                // the chord fall through to the embedder, whose
                // image-paste bridge hangs off exactly that fall-through.
                match clipboard.read(ClipboardKind::Standard) {
                    Some(data) if !data.is_empty() => {
                        return Some(Command::Write(paste_content(
                            &last_content.terminal_mode,
                            &data,
                        )));
                    },
                    _ => {},
                }
            },
            BindingAction::Copy => {
                clipboard.write(
                    ClipboardKind::Standard,
                    self.term.backend.selectable_content(),
                );
            },
            _ => {},
        };

        None
    }
}

impl Widget<Event, Theme, iced::Renderer> for TerminalView<'_> {
    fn size(&self) -> Size<Length> {
        Size {
            width: Length::Fill,
            height: Length::Fill,
        }
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<TerminalViewState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(TerminalViewState::new())
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &iced::Renderer,
        limits: &iced_core::layout::Limits,
    ) -> iced_core::layout::Node {
        let size = limits.resolve(Length::Fill, Length::Fill, Size::ZERO);
        iced::advanced::layout::Node::new(size)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: iced_core::Layout<'_>,
        _renderer: &iced::Renderer,
        operation: &mut dyn operation::Operation,
    ) {
        let state = tree.state.downcast_mut::<TerminalViewState>();
        let wid = self.term.widget_id();
        operation.focusable(Some(wid), layout.bounds(), state);
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        _theme: &Theme,
        _style: &iced::advanced::renderer::Style,
        layout: iced::advanced::Layout,
        _cursor: Cursor,
        viewport: &Rectangle,
    ) {
        let draw_started = std::time::Instant::now();
        let state = tree.state.downcast_ref::<TerminalViewState>();
        let content = self.term.backend.renderable_content();
        let term_size = content.terminal_size;
        let cell_width = term_size.cell_width;
        let cell_height = term_size.cell_height;
        let font_size = self.term.font.size;
        let font_scale_factor = self.term.font.scale_factor;
        // with_clip only MASKS to `bounds` — the child frame keeps the
        // parent's coordinate space (identity transform, no translation to
        // the region origin), so cells must keep their absolute positions.
        // The mask stops a partial bottom row from painting outside the
        // terminal's bounds.
        let bounds = layout.bounds();
        let layout_offset_x = bounds.x;
        let layout_offset_y = bounds.y;

        let geom = self.term.cache.draw(renderer, viewport.size(), |frame| {
            frame.with_clip(bounds, |frame| {
                // Precompute constants used in the inner loop
                let display_offset = content.display_offset as f32;
                let cell_size = Size::new(cell_width, cell_height);
                let half_w = cell_width * 0.5;
                let half_h = cell_height * 0.5;
                // We use the background pallete color as a default
                // because the widget global background color must be the same
                let default_bg = self
                    .term
                    .theme
                    .get_color(ansi::Color::Named(NamedColor::Background));

                let mut last_line: Option<i32> = None;
                let mut bg_batch_rect = BackgroundRect::default();
                let base_font = self.term.font.font_type;
                let mut text_run: Option<TextRun> = None;

                for indexed in &content.cells {
                    // Compute per-cell geometry cheaply
                    let line = indexed.point.line.0;
                    let col = indexed.point.column.0 as f32;

                    // Resolve position point for this cell
                    let x = layout_offset_x + (col * cell_width);
                    let y = layout_offset_y
                        + (((line as f32) + display_offset) * cell_height);
                    let cell_center_y = y + half_h;
                    let cell_center_x = x + half_w;

                    // Resolve colors for this cell
                    let mut fg = self.term.theme.get_color(indexed.fg);
                    let mut bg = self.term.theme.get_color(indexed.bg);

                    // If the new line was detected,
                    // need to flush pending background rect and init the new one
                    if last_line != Some(line) {
                        if bg_batch_rect.can_flush() {
                            let line = last_line.unwrap_or(line);
                            frame.fill(
                                &bg_batch_rect.build(line),
                                bg_batch_rect.color,
                            );
                        }

                        last_line = Some(line);
                        bg_batch_rect = BackgroundRect::default()
                            .with_cell_height(cell_height)
                            .with_display_offset(display_offset)
                            .with_layout_offset_y(layout_offset_y);
                    }

                    // Handle dim, inverse, and selected text
                    if indexed
                        .cell
                        .flags
                        .intersects(cell::Flags::DIM | cell::Flags::DIM_BOLD)
                    {
                        fg.a *= 0.7;
                    }
                    if indexed.cell.flags.contains(cell::Flags::INVERSE)
                        || content
                            .selectable_range
                            .is_some_and(|r| r.contains(indexed.point))
                        // Visible search matches highlight like a
                        // selection.
                        || content
                            .search_matches
                            .iter()
                            .any(|r| r.contains(&indexed.point))
                    {
                        std::mem::swap(&mut fg, &mut bg);
                    }

                    // Batch draw backgrounds: skip default background (container already paints it)
                    if bg != default_bg {
                        if bg_batch_rect.can_extend(bg, x) {
                            // Same color and contiguous: extend current run
                            bg_batch_rect.extend(cell_width);
                        } else {
                            // New colored run (or non-contiguous): flush previous run if any
                            if bg_batch_rect.can_flush() {
                                frame.fill(
                                    &bg_batch_rect.build(line),
                                    bg_batch_rect.color,
                                );
                            }

                            // Start a new run but do not draw yet; wait for potential extensions
                            bg_batch_rect = BackgroundRect::default()
                                .with_cell_height(cell_height)
                                .with_display_offset(display_offset)
                                .with_layout_offset_y(layout_offset_y)
                                .activate()
                                .with_color(bg)
                                .with_start_x(x)
                                .with_width(cell_width);
                        }
                    } else if bg_batch_rect.can_flush() {
                        // Background returns to default, flush current background rect and init the new one
                        frame.fill(
                            &bg_batch_rect.build(line),
                            bg_batch_rect.color,
                        );

                        bg_batch_rect = BackgroundRect::default()
                            .with_cell_height(cell_height)
                            .with_display_offset(display_offset)
                            .with_layout_offset_y(layout_offset_y);
                    }

                    // Draw hovered hyperlink underline (rare; keep per-cell for correctness)
                    if content.hovered_hyperlink.as_ref().is_some_and(|range| {
                        range.contains(&indexed.point)
                            && range.contains(&state.mouse_position_on_grid)
                    }) || indexed.cell.flags.contains(cell::Flags::UNDERLINE)
                    {
                        let underline_height = y + cell_size.height;
                        let underline = Path::line(
                            Point::new(x, underline_height),
                            Point::new(x + cell_size.width, underline_height),
                        );
                        frame.stroke(
                            &underline,
                            Stroke::default()
                                .with_width(font_size * 0.15)
                                .with_color(fg),
                        );
                    }

                    // Handle cursor rendering
                    if content.cursor_point == indexed.point
                        && content.terminal_mode.contains(TermMode::SHOW_CURSOR)
                    {
                        let cursor_color =
                            self.term.theme.get_color(content.cursor.fg);
                        let cursor_rect =
                            Path::rectangle(Point::new(x, y), cell_size);
                        frame.fill(&cursor_rect, cursor_color);
                    }

                    // Draw text: contiguous same-style ASCII cells
                    // merge into one fill_text run (thousands of calls per
                    // frame become dozens) with cheap Basic shaping. The
                    // cursor cell and non-ASCII take the per-cell path with
                    // full shaping. Valid because cell_width IS the font
                    // advance (font.rs measures the same text pipeline), so
                    // a Left-aligned run lands every glyph on its cell.
                    let is_cursor_cell = content.cursor_point == indexed.point;
                    if indexed.c != ' ' && indexed.c != '\t' {
                        if is_cursor_cell
                            && content
                                .terminal_mode
                                .contains(TermMode::APP_CURSOR)
                        {
                            fg = bg;
                        }
                        let bold = indexed.cell.flags.intersects(
                            cell::Flags::BOLD | cell::Flags::DIM_BOLD,
                        );
                        let italic =
                            indexed.cell.flags.contains(cell::Flags::ITALIC);

                        if indexed.c.is_ascii_graphic() && !is_cursor_cell {
                            let col = indexed.point.column.0;
                            let extended = match &mut text_run {
                                Some(run)
                                    if run.can_extend(
                                        line, col, fg, bold, italic,
                                    ) =>
                                {
                                    run.push(indexed.c);
                                    true
                                },
                                _ => false,
                            };
                            if !extended {
                                if let Some(run) = text_run.take() {
                                    run.fill(
                                        frame,
                                        base_font,
                                        font_size,
                                        font_scale_factor,
                                    );
                                }
                                text_run = Some(TextRun::start(
                                    indexed.c,
                                    x,
                                    cell_center_y,
                                    line,
                                    col,
                                    fg,
                                    bold,
                                    italic,
                                ));
                            }
                        } else {
                            if let Some(run) = text_run.take() {
                                run.fill(
                                    frame,
                                    base_font,
                                    font_size,
                                    font_scale_factor,
                                );
                            }
                            let mut font = base_font;
                            if bold {
                                font.weight = FontWeight::Bold;
                            }
                            if italic {
                                font.style = FontStyle::Italic;
                            }
                            frame.fill_text(Text {
                                content: indexed.cell.c.to_string(),
                                position: Point::new(
                                    cell_center_x,
                                    cell_center_y,
                                ),
                                font,
                                size: iced_core::Pixels(font_size),
                                color: fg,
                                align_x: Alignment::Center,
                                align_y: Vertical::Center,
                                shaping: Shaping::Advanced,
                                line_height: LineHeight::Relative(
                                    font_scale_factor,
                                ),
                                ..Default::default()
                            });
                        }
                    } else if let Some(run) = &mut text_run {
                        // Spaces extend an active run (monospace advance)
                        // instead of fragmenting it per word.
                        if run.can_extend_space(line, indexed.point.column.0) {
                            run.push(' ');
                        } else if let Some(run) = text_run.take() {
                            run.fill(
                                frame,
                                base_font,
                                font_size,
                                font_scale_factor,
                            );
                        }
                    }
                }

                if let Some(run) = text_run.take() {
                    run.fill(frame, base_font, font_size, font_scale_factor);
                }

                // Flush any remaining background run at the end
                if bg_batch_rect.can_flush() {
                    frame.fill(
                        &bg_batch_rect.build(last_line.unwrap_or(0)),
                        bg_batch_rect.color,
                    );
                }
            });
        });

        use iced::advanced::graphics::geometry::Renderer as _;
        renderer.draw_geometry(geom);

        let dt = draw_started.elapsed();
        if dt.as_millis() > 20 {
            eprintln!(
                "[perf] draw tab {} took {:?} ({} cells)",
                self.term.id,
                dt,
                self.term.backend.renderable_content().cells.len()
            );
        }
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &iced_core::Event,
        layout: iced_graphics::core::Layout<'_>,
        cursor: Cursor,
        _renderer: &iced::Renderer,
        clipboard: &mut dyn iced_graphics::core::Clipboard,
        shell: &mut iced_graphics::core::Shell<'_, Event>,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<TerminalViewState>();
        self.handle_resize(state, layout, shell);

        let is_cursor_in_layout = self.is_cursor_in_layout(cursor, layout);
        self.handle_focus(event, state, is_cursor_in_layout);

        // A drag that leaves the widget must still see its release —
        // otherwise the drag state sticks and a selection finished outside
        // the bounds is never offered to PRIMARY.
        let is_dragged_release = state.is_dragged
            && matches!(
                event,
                iced::Event::Mouse(iced_core::mouse::Event::ButtonReleased(
                    iced_core::mouse::Button::Left
                ))
            );

        let commands = match event {
            iced::Event::Mouse(mouse_event)
                if is_cursor_in_layout || is_dragged_release =>
            {
                self.handle_mouse_event(
                    state,
                    clipboard,
                    layout.position(),
                    cursor.position().unwrap_or_default(),
                    mouse_event,
                )
            },
            iced::Event::Keyboard(keyboard_event) => {
                if !state.is_focused() {
                    return;
                }

                self.handle_keyboard_event(state, clipboard, keyboard_event)
                    .into_iter()
                    .collect()
            },
            _ => Vec::new(),
        };

        if !commands.is_empty() {
            shell.capture_event();
        }

        for cmd in commands {
            shell.publish(Event::BackendCall(self.term.id, cmd));
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: iced_core::Layout<'_>,
        cursor: iced_core::mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> iced_core::mouse::Interaction {
        let state = tree.state.downcast_ref::<TerminalViewState>();
        let mut cursor_mode = iced_core::mouse::Interaction::Idle;
        let terminal_mode =
            self.term.backend.renderable_content().terminal_mode;
        if self.is_cursor_in_layout(cursor, layout)
            && !terminal_mode.contains(TermMode::SGR_MOUSE)
        {
            cursor_mode = iced_core::mouse::Interaction::Text;
        }

        if self.is_cursor_hovered_hyperlink(state) {
            cursor_mode = iced_core::mouse::Interaction::Pointer;
        }

        cursor_mode
    }
}

impl<'a> From<TerminalView<'a>> for Element<'a, Event, Theme, iced::Renderer> {
    fn from(widget: TerminalView<'a>) -> Self {
        Self::new(widget)
    }
}

#[derive(Debug, Clone)]
struct TerminalViewState {
    focus: bool,
    is_dragged: bool,
    drag_is_mouse_report: bool,
    last_click: Option<mouse::Click>,
    scroll_pixels: f32,
    keyboard_modifiers: Modifiers,
    size: Size<f32>,
    /// Which terminal `size` was last sent to — state is reused by TYPE at
    /// a tree position, so a different terminal in the same slot must not
    /// inherit "already sized".
    sized_for: Option<u64>,
    mouse_position_on_grid: TerminalGridPoint,
}

impl TerminalViewState {
    fn new() -> Self {
        Self {
            focus: false,
            is_dragged: false,
            drag_is_mouse_report: false,
            last_click: None,
            scroll_pixels: 0.0,
            keyboard_modifiers: Modifiers::empty(),
            size: Size::from([0.0, 0.0]),
            sized_for: None,
            mouse_position_on_grid: TerminalGridPoint::default(),
        }
    }
}

impl Default for TerminalViewState {
    fn default() -> Self {
        Self::new()
    }
}

impl operation::Focusable for TerminalViewState {
    fn is_focused(&self) -> bool {
        self.focus
    }

    fn focus(&mut self) {
        self.focus = true;
    }

    fn unfocus(&mut self) {
        self.focus = false;
    }
}

/// The no-binding fallback shared by both keyboard arms: the text the key
/// itself produced, ESC-prefixed when Alt is held (xterm metaSendsEscape,
/// the VTE and alacritty defaults) — single-byte input only, so
/// AltGr-composed characters pass unmangled. `None` when the key produced
/// no text (F-keys, arrows): those write nothing without a binding.
fn text_fallback(text: Option<&str>, alt: bool) -> Option<Vec<u8>> {
    let text = text?;
    if text.is_empty() {
        return None;
    }
    let mut bytes = text.as_bytes().to_vec();
    if alt && bytes.len() == 1 {
        bytes.insert(0, 0x1b);
    }
    Some(bytes)
}

/// Prepare clipboard text for the PTY the way VTE/alacritty do: wrap in the
/// bracketed-paste markers when the app opted in (stripping an embedded end
/// marker — paste injection guard), otherwise normalize newlines to carriage
/// returns so a shell runs the lines instead of literal-inserting them.
fn paste_content(mode: &TermMode, data: &str) -> Vec<u8> {
    if mode.contains(TermMode::BRACKETED_PASTE) {
        let mut out = b"\x1b[200~".to_vec();
        out.extend_from_slice(data.replace("\x1b[201~", "").as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        data.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

/// Contiguous same-style ASCII cells merged into a single text fill.
struct TextRun {
    content: String,
    start_x: f32,
    center_y: f32,
    line: i32,
    next_col: usize,
    fg: Color,
    bold: bool,
    italic: bool,
}

impl TextRun {
    #[allow(clippy::too_many_arguments)]
    fn start(
        c: char,
        x: f32,
        center_y: f32,
        line: i32,
        col: usize,
        fg: Color,
        bold: bool,
        italic: bool,
    ) -> Self {
        Self {
            content: c.to_string(),
            start_x: x,
            center_y,
            line,
            next_col: col + 1,
            fg,
            bold,
            italic,
        }
    }

    fn can_extend(
        &self,
        line: i32,
        col: usize,
        fg: Color,
        bold: bool,
        italic: bool,
    ) -> bool {
        self.line == line
            && self.next_col == col
            && self.fg == fg
            && self.bold == bold
            && self.italic == italic
    }

    fn can_extend_space(&self, line: i32, col: usize) -> bool {
        self.line == line && self.next_col == col
    }

    fn push(&mut self, c: char) {
        self.content.push(c);
        self.next_col += 1;
    }

    fn fill(
        self,
        frame: &mut iced::widget::canvas::Frame,
        mut font: iced::Font,
        font_size: f32,
        scale_factor: f32,
    ) {
        if self.bold {
            font.weight = FontWeight::Bold;
        }
        if self.italic {
            font.style = FontStyle::Italic;
        }
        frame.fill_text(Text {
            content: self.content,
            position: Point::new(self.start_x, self.center_y),
            font,
            size: iced_core::Pixels(font_size),
            color: self.fg,
            align_x: Alignment::Left,
            align_y: Vertical::Center,
            shaping: Shaping::Basic,
            line_height: LineHeight::Relative(scale_factor),
            ..Default::default()
        });
    }
}

#[derive(Default)]
struct BackgroundRect {
    display_offset: f32,
    cell_height: f32,
    layout_offset_y: f32,
    is_active: bool,
    color: Color,
    start_x: f32,
    width: f32,
}

impl BackgroundRect {
    fn with_display_offset(mut self, value: f32) -> Self {
        self.display_offset = value;
        self
    }

    fn with_cell_height(mut self, value: f32) -> Self {
        self.cell_height = value;
        self
    }

    fn with_layout_offset_y(mut self, value: f32) -> Self {
        self.layout_offset_y = value;
        self
    }

    fn with_width(mut self, value: f32) -> Self {
        self.width = value;
        self
    }

    fn with_start_x(mut self, value: f32) -> Self {
        self.start_x = value;
        self
    }

    fn with_color(mut self, value: Color) -> Self {
        self.color = value;
        self
    }

    fn activate(mut self) -> Self {
        self.is_active = true;
        self
    }

    fn build(&self, line: i32) -> Path {
        let flush_y = self.layout_offset_y
            + ((line as f32 + self.display_offset) * self.cell_height);
        Path::rectangle(
            Point::new(self.start_x, flush_y),
            Size::new(self.width, self.cell_height),
        )
    }

    fn can_flush(&self) -> bool {
        self.is_active && self.width > 0.0
    }

    fn can_extend(&self, bg: Color, x: f32) -> bool {
        self.is_active
            && bg == self.color
            && (self.start_x + self.width - x).abs() < f32::EPSILON
    }

    fn extend(&mut self, value: f32) {
        self.width += value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shared no-binding fallback: a modified-but-unbound key still
    /// types what it produced. Space-with-Shift-held is the case that
    /// found it — fast prose typing (a capital, then the spacebar)
    /// silently swallowed the space.
    #[test]
    fn text_fallback_types_what_the_key_produced() {
        assert_eq!(text_fallback(Some(" "), false), Some(b" ".to_vec()));
        // Alt ESC-prefixes single-byte input (metaSendsEscape)...
        assert_eq!(text_fallback(Some(" "), true), Some(b"\x1b ".to_vec()));
        // ...but multi-byte (AltGr compositions) pass unmangled.
        assert_eq!(
            text_fallback(Some("\u{20ac}"), true),
            Some("\u{20ac}".as_bytes().to_vec())
        );
        // No text (F-keys, arrows) or empty text writes nothing.
        assert_eq!(text_fallback(None, false), None);
        assert_eq!(text_fallback(Some(""), false), None);
    }

    mod handle_left_button_pressed_tests {
        use super::*;
        use alacritty_terminal::index::{Column, Line};

        #[test]
        fn handles_mouse_mode_with_left_click() {
            let mut state = TerminalViewState::new();
            let terminal_mode = TermMode::MOUSE_MODE;
            let layout_position = Point { x: 5.0, y: 5.0 };
            let cursor_position = Point { x: 100.0, y: 150.0 };
            let mut commands = Vec::new();
            let _modifiers = Modifiers::empty();

            TerminalView::handle_left_button_pressed(
                &mut state,
                &terminal_mode,
                cursor_position,
                layout_position,
                &mut commands,
            );

            assert_eq!(commands.len(), 1);
            assert!(matches!(
                commands[0],
                Command::MouseReport(
                    MouseButton::LeftButton,
                    _modifiers,
                    TerminalGridPoint {
                        line: Line(0),
                        column: Column(0),
                    },
                    true,
                )
            ));
            assert!(state.is_dragged);
        }

        #[test]
        fn starts_simple_selection_with_left_click() {
            let terminal_mode = TermMode::SGR_MOUSE;
            let cursor_position = Point { x: 200.0, y: 150.0 };
            let layout_position = Point { x: 50.0, y: 50.0 };

            let cases = vec![
                SelectionType::Simple,
                SelectionType::Semantic,
                SelectionType::Lines,
            ];

            for _selection_type in cases {
                let mut state = TerminalViewState::new();
                state.keyboard_modifiers = Modifiers::SHIFT;
                let mut commands = Vec::new();

                TerminalView::handle_left_button_pressed(
                    &mut state,
                    &terminal_mode,
                    cursor_position,
                    layout_position,
                    &mut commands,
                );

                assert_eq!(commands.len(), 1);
                assert!(matches!(
                    commands[0],
                    Command::SelectStart(_selection_type, (150.0, 100.0))
                ),);
                assert!(state.is_dragged);
            }
        }
    }

    mod handle_cursor_moved_tests {
        use alacritty_terminal::index::{Column, Line};

        use super::*;

        #[test]
        fn updates_mouse_position_on_grid() {
            let mut state = TerminalViewState::new();
            let terminal_content = RenderableContent::default();
            let mut commands = Vec::new();
            let cases = vec![
                (
                    Point { x: 0.0, y: 0.0 },
                    Point { x: 1.0, y: 1.0 },
                    TerminalGridPoint {
                        line: Line(1),
                        column: Column(1),
                    },
                ),
                (
                    Point { x: 0.0, y: 0.0 },
                    Point { x: 2.0, y: 2.0 },
                    TerminalGridPoint {
                        line: Line(2),
                        column: Column(2),
                    },
                ),
                (
                    Point { x: 0.0, y: 0.0 },
                    Point { x: 30.0, y: 2.0 },
                    TerminalGridPoint {
                        line: Line(2),
                        column: Column(30),
                    },
                ),
                (
                    Point { x: 10.0, y: 0.0 },
                    Point { x: 30.0, y: 2.0 },
                    TerminalGridPoint {
                        line: Line(2),
                        column: Column(20),
                    },
                ),
                (
                    Point { x: 10.0, y: 10.0 },
                    Point { x: 30.0, y: 2.0 },
                    TerminalGridPoint {
                        line: Line(0),
                        column: Column(20),
                    },
                ),
            ];

            for (layout_position, cursor_position, expected) in cases {
                TerminalView::handle_cursor_moved(
                    &mut state,
                    &terminal_content,
                    &cursor_position,
                    layout_position,
                    &mut commands,
                );

                assert_eq!(state.mouse_position_on_grid, expected);
            }
        }

        #[test]
        fn generates_drag_update_command_when_dragged() {
            let mut state = TerminalViewState::new();
            state.is_dragged = true; // Simulate an ongoing drag operation
            let terminal_content = RenderableContent::default();
            let layout_position = Point { x: 5.0, y: 5.0 };
            let cursor_position = Point { x: 100.0, y: 150.0 };
            let mut commands = Vec::new();

            TerminalView::handle_cursor_moved(
                &mut state,
                &terminal_content,
                &cursor_position,
                layout_position,
                &mut commands,
            );

            assert_eq!(commands.len(), 1);
            assert!(matches!(
                commands[0],
                Command::SelectUpdate((95.0, 145.0))
            ));
        }

        #[test]
        fn generates_drag_update_command_when_dragged_in_mouse_motion_mode() {
            let mut state = TerminalViewState::new();
            state.is_dragged = true; // Simulate an ongoing drag operation
            state.drag_is_mouse_report = true;
            let mut terminal_content = RenderableContent::default();
            terminal_content.terminal_mode = TermMode::MOUSE_MOTION;
            let layout_position = Point { x: 5.0, y: 5.0 };
            let cursor_position = Point { x: 100.0, y: 150.0 };
            let mut commands = Vec::new();
            let _modifiers = Modifiers::empty();

            TerminalView::handle_cursor_moved(
                &mut state,
                &terminal_content,
                &cursor_position,
                layout_position,
                &mut commands,
            );

            assert_eq!(commands.len(), 1);
            assert!(matches!(
                commands[0],
                Command::MouseReport(
                    MouseButton::LeftMove,
                    _modifiers,
                    TerminalGridPoint {
                        line: Line(49),
                        column: Column(79),
                    },
                    true,
                )
            ));
        }

        #[test]
        fn generates_drag_update_command_when_dragged_in_srg_mode_with_key_mods(
        ) {
            let mut state = TerminalViewState::new();
            state.keyboard_modifiers = Modifiers::SHIFT;
            state.is_dragged = true; // Simulate an ongoing drag operation
            let mut terminal_content = RenderableContent::default();
            terminal_content.terminal_mode = TermMode::SGR_MOUSE;
            let layout_position = Point { x: 5.0, y: 5.0 };
            let cursor_position = Point { x: 100.0, y: 150.0 };
            let mut commands = Vec::new();

            TerminalView::handle_cursor_moved(
                &mut state,
                &terminal_content,
                &cursor_position,
                layout_position,
                &mut commands,
            );

            assert_eq!(commands.len(), 1);
            assert!(matches!(
                commands[0],
                Command::SelectUpdate((95.0, 145.0))
            ));
        }

        #[test]
        fn generates_drag_update_and_link_open() {
            let mut state = TerminalViewState::new();
            state.keyboard_modifiers = Modifiers::COMMAND;
            state.is_dragged = true; // Simulate an ongoing drag operation
            let mut terminal_content = RenderableContent::default();
            terminal_content.terminal_mode = TermMode::SGR_MOUSE;
            let layout_position = Point { x: 5.0, y: 5.0 };
            let cursor_position = Point { x: 100.0, y: 150.0 };
            let mut commands = Vec::new();

            TerminalView::handle_cursor_moved(
                &mut state,
                &terminal_content,
                &cursor_position,
                layout_position,
                &mut commands,
            );

            assert_eq!(commands.len(), 2);
            assert!(matches!(
                commands[0],
                Command::SelectUpdate((95.0, 145.0))
            ));
            assert!(matches!(
                commands[1],
                Command::ProcessLink(
                    LinkAction::Hover,
                    TerminalGridPoint {
                        line: Line(49),
                        column: Column(79),
                    },
                )
            ));
        }
    }

    mod handle_button_released_tests {
        use super::*;
        use alacritty_terminal::index::{Column, Line};

        #[test]
        fn mouse_mode_activated() {
            let mut state = TerminalViewState::new();
            state.drag_is_mouse_report = true;
            let terminal_mode = TermMode::MOUSE_MODE;
            let bindings = BindingsLayout::new();
            let mut commands = Vec::new();
            let _modifiers = Modifiers::empty();

            TerminalView::handle_button_released(
                &mut state,
                &terminal_mode,
                &bindings,
                &mut commands,
            );

            assert_eq!(commands.len(), 1);
            assert!(matches!(
                commands[0],
                Command::MouseReport(
                    MouseButton::LeftButton,
                    _modifiers,
                    TerminalGridPoint {
                        line: Line(0),
                        column: Column(0)
                    },
                    false
                )
            ));
        }

        #[test]
        fn link_open_on_button_release() {
            let mut state = TerminalViewState::new();
            state.drag_is_mouse_report = true;
            state.keyboard_modifiers = Modifiers::COMMAND;
            let terminal_mode = TermMode::MOUSE_MODE;
            let bindings = BindingsLayout::new();
            let mut commands = Vec::new();
            let _modifiers = Modifiers::empty();

            TerminalView::handle_button_released(
                &mut state,
                &terminal_mode,
                &bindings,
                &mut commands,
            );

            assert_eq!(commands.len(), 2);
            assert!(matches!(
                commands[0],
                Command::MouseReport(
                    MouseButton::LeftButton,
                    _modifiers,
                    TerminalGridPoint {
                        line: Line(0),
                        column: Column(0)
                    },
                    false
                )
            ));
            assert!(matches!(
                commands[1],
                Command::ProcessLink(
                    LinkAction::Open,
                    TerminalGridPoint {
                        line: Line(0),
                        column: Column(0)
                    }
                ),
            ));
        }

        #[test]
        fn selection_drag_release_requests_primary_publish() {
            let mut state = TerminalViewState::new();
            state.is_dragged = true;
            state.drag_is_mouse_report = false;
            let terminal_mode = TermMode::empty();
            let bindings = BindingsLayout::new();
            let mut commands = Vec::new();

            TerminalView::handle_button_released(
                &mut state,
                &terminal_mode,
                &bindings,
                &mut commands,
            );

            assert_eq!(commands.len(), 1);
            assert!(matches!(commands[0], Command::SelectRelease));
            assert!(!state.is_dragged);
        }

        #[test]
        fn report_drag_release_does_not_publish_selection() {
            let mut state = TerminalViewState::new();
            state.is_dragged = true;
            state.drag_is_mouse_report = true;
            let terminal_mode = TermMode::MOUSE_MODE;
            let bindings = BindingsLayout::new();
            let mut commands = Vec::new();

            TerminalView::handle_button_released(
                &mut state,
                &terminal_mode,
                &bindings,
                &mut commands,
            );

            assert!(!commands
                .iter()
                .any(|c| matches!(c, Command::SelectRelease)));
        }

        #[test]
        fn link_open_on_button_release_in_non_mouse_mode() {
            let mut state = TerminalViewState::new();
            state.keyboard_modifiers = Modifiers::COMMAND;
            state.mouse_position_on_grid = TerminalGridPoint {
                line: Line(4),
                column: Column(10),
            };
            let terminal_mode = TermMode::empty(); // Assume SGR_MOUSE mode doesn't affect link opening
            let bindings = BindingsLayout::new();
            let mut commands = Vec::new();

            TerminalView::handle_button_released(
                &mut state,
                &terminal_mode,
                &bindings,
                &mut commands,
            );

            assert_eq!(commands.len(), 1);
            assert!(matches!(
                commands[0],
                Command::ProcessLink(
                    LinkAction::Open,
                    TerminalGridPoint {
                        line: Line(4),
                        column: Column(10)
                    }
                ),
            ));
        }
    }

    mod paste_content_tests {
        use super::*;

        #[test]
        fn bracketed_mode_wraps_and_strips_end_marker() {
            let mode = TermMode::BRACKETED_PASTE;
            let out = paste_content(&mode, "safe\x1b[201~rm -rf /\n");
            assert_eq!(out, b"\x1b[200~saferm -rf /\n\x1b[201~".to_vec());
        }

        #[test]
        fn plain_mode_normalizes_newlines() {
            let mode = TermMode::empty();
            let out = paste_content(&mode, "line1\r\nline2\nline3");
            assert_eq!(out, b"line1\rline2\rline3".to_vec());
        }
    }

    mod handle_wheel_scrolled_tests {
        use super::*;
        use crate::font::TermFont;
        use crate::settings::FontSettings;

        #[test]
        fn scroll_with_lines_downward() {
            let mut state = TerminalViewState::new();
            let font = TermFont::new(FontSettings::default());
            let mut commands = Vec::new();

            TerminalView::handle_wheel_scrolled(
                &mut state,
                ScrollDelta::Lines { y: 3.0, x: 0.0 }, // Scroll down 3 lines
                &TermMode::empty(),
                &font.measure,
                &mut commands,
            );

            assert_eq!(commands.len(), 1);
            assert!(matches!(commands[0], Command::Scroll(3)));
        }

        #[test]
        fn scroll_with_lines_upward() {
            let mut state = TerminalViewState::new();
            let font = TermFont::new(FontSettings::default());
            let mut commands = Vec::new();

            TerminalView::handle_wheel_scrolled(
                &mut state,
                ScrollDelta::Lines { y: -2.0, x: 0.0 },
                &TermMode::empty(),
                &font.measure,
                &mut commands,
            );

            assert_eq!(commands.len(), 1);
            assert!(matches!(commands[0], Command::Scroll(-2)));
        }

        #[test]
        fn scroll_with_pixels_accumulating_downward() {
            let mut state = TerminalViewState::new();
            let font = TermFont::new(FontSettings::default());
            let mut commands = Vec::new();

            TerminalView::handle_wheel_scrolled(
                &mut state,
                ScrollDelta::Pixels { y: 45.0, x: 0.0 },
                &TermMode::empty(),
                &font.measure,
                &mut commands,
            );

            assert_eq!(commands.len(), 1);
            assert!(matches!(commands[0], Command::Scroll(-2)));
            assert_eq!(state.scroll_pixels, -8.600002);
        }

        #[test]
        fn scroll_with_pixels_accumulating_upward() {
            let mut state = TerminalViewState::new();
            let font = TermFont::new(FontSettings::default());
            let mut commands = Vec::new();

            TerminalView::handle_wheel_scrolled(
                &mut state,
                ScrollDelta::Pixels { y: -60.0, x: 0.0 },
                &TermMode::empty(),
                &font.measure,
                &mut commands,
            );

            assert_eq!(commands.len(), 1);
            assert!(matches!(commands[0], Command::Scroll(3)));
            assert_eq!(state.scroll_pixels, 5.4000034);
        }
    }
}
