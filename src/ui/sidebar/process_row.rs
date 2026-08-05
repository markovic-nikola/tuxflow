use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{cairo, gio, glib};

use crate::config::schema::ProcessCategory;
use crate::process::manager::ProcessStatus;

type ActionCallback = Rc<RefCell<Option<Box<dyn Fn(&str, &str)>>>>;

/// Working-state indicator animation (GTK CSS has no transform, so the
/// morph + spin is cairo-drawn on a DrawingArea, advanced by a frame-clock
/// tick): the dot morphs into a square, the square spins with a pace that
/// eases fast-slow-fast, and morphing back to a circle when work ends.
struct WorkingAnim {
    /// Target state: true while the agent is producing output.
    active: Cell<bool>,
    /// 0.0 = circle, 1.0 = square.
    morph: Cell<f64>,
    /// Current rotation in radians.
    angle: Cell<f64>,
    /// Time accumulator driving the speed oscillation.
    phase: Cell<f64>,
    /// Last frame-clock timestamp (µs); i64::MIN = no frame yet.
    last_us: Cell<i64>,
    /// Guards against stacking multiple tick callbacks.
    ticking: Cell<bool>,
}

/// Seconds for the circle<->square morph.
const MORPH_SECS: f64 = 0.25;
/// Each spin cycle bursts up to SPIN_FAST, then winds down toward
/// SPIN_SLOW (rad/s) for the rest of the cycle, repeating until done.
const SPIN_FAST: f64 = 11.0;
/// Floor is kept visibly turning — at 0.3 the wind-down read as a full stop.
const SPIN_SLOW: f64 = 1.6;
/// Seconds per burst-and-wind-down cycle.
const SPIN_CYCLE_SECS: f64 = 2.2;
/// Fraction of the cycle spent accelerating (the burst).
const SPIN_ATTACK: f64 = 0.15;

pub struct ProcessRow {
    container: gtk4::Box,
    status_dot: gtk4::Label,
    status_stack: gtk4::Stack,
    status_area: gtk4::DrawingArea,
    anim: Rc<WorkingAnim>,
    is_terminal: bool,
    is_running: Cell<bool>,
    name_label: gtk4::Label,
    /// Shared name used by button callbacks so they track renames.
    action_name: Rc<RefCell<String>>,
    /// Shared qualified name (project::process) for context actions.
    pub qualified_name: Rc<RefCell<String>>,
    keybind_label: gtk4::Label,
    port_label: gtk4::Label,
    browser_button: gtk4::Button,
    play_button: gtk4::Button,
    restart_button: gtk4::Button,
    stop_button: gtk4::Button,
    on_context_action: ActionCallback,
    url: Rc<RefCell<Option<String>>>,
    /// Menu model section that mirrors URL state in the right-click popover.
    /// Populated eagerly because `set_url` runs before the popover is opened.
    /// When the popover is later built, the section is wired into the model.
    browser_menu_section: gio::Menu,
}

impl ProcessRow {
    pub fn new(name: &str, command: &str, category: ProcessCategory) -> Self {
        Self::new_with_options(name, command, false, category)
    }

    pub fn new_terminal(name: &str, command: &str) -> Self {
        // Terminal/SSH rows pass their category here; resume action is gated
        // on Agent only, so the exact value doesn't matter for the menu.
        Self::new_with_options(name, command, true, ProcessCategory::Terminal)
    }

    fn new_with_options(
        name: &str,
        command: &str,
        is_terminal: bool,
        category: ProcessCategory,
    ) -> Self {
        let container = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        container.set_margin_start(8);
        container.set_margin_end(12);
        container.set_margin_top(4);
        container.set_margin_bottom(4);
        container.add_css_class("process-row");
        container.set_tooltip_text(Some(command));

        // Status indicator: a Stack switching between the status dot and the
        // cairo-drawn working animation. Both 14px wide so nothing shifts.
        let status_dot = gtk4::Label::builder()
            .label("\u{25CF}") // ●
            .width_request(14)
            .css_classes(["caption", "status-stopped"])
            .build();

        // status-working only supplies the color (amber, or project yellow
        // in remote projects) — resolved via CSS so theming stays in one place.
        let status_area = gtk4::DrawingArea::builder()
            .content_width(14)
            .content_height(14)
            .valign(gtk4::Align::Center)
            .css_classes(["status-working"])
            .build();

        let anim = Rc::new(WorkingAnim {
            active: Cell::new(false),
            morph: Cell::new(0.0),
            angle: Cell::new(0.0),
            phase: Cell::new(0.0),
            last_us: Cell::new(i64::MIN),
            ticking: Cell::new(false),
        });

        let draw_anim = anim.clone();
        status_area.set_draw_func(move |area, cr, w, h| {
            let m = draw_anim.morph.get();
            let color = area.color();
            cr.save().ok();
            cr.translate(w as f64 / 2.0, h as f64 / 2.0);
            cr.rotate(draw_anim.angle.get());
            // Same width as the resting dot; corner radius runs from
            // half-side (= circle) down to 0 (sharp-cornered square).
            let half = 4.0;
            let radius = half * (1.0 - m);
            rounded_square(cr, half, radius);
            let (r, g, b, a) = (
                color.red() as f64,
                color.green() as f64,
                color.blue() as f64,
                color.alpha() as f64,
            );
            // Hollow square: the fill drains out with the morph, leaving
            // just the outline; it fills back in on the way to the dot.
            cr.set_source_rgba(r, g, b, a * (1.0 - m));
            cr.fill_preserve().ok();
            cr.set_line_width(1.4);
            cr.set_source_rgba(r, g, b, a);
            cr.stroke().ok();
            cr.restore().ok();
        });

        let status_stack = gtk4::Stack::builder().valign(gtk4::Align::Center).build();
        status_stack.add_named(&status_dot, Some("dot"));
        status_stack.add_named(&status_area, Some("working"));

        container.append(&status_stack);

        // Process name
        let name_label = gtk4::Label::builder()
            .label(name)
            .halign(gtk4::Align::Start)
            .hexpand(true)
            .ellipsize(gtk4::pango::EllipsizeMode::End)
            .build();
        container.append(&name_label);

        // CPU label (hidden by default)

        // Port label (hidden by default)
        let port_label = gtk4::Label::builder()
            .css_classes(["caption", "dim-label"])
            .visible(false)
            .build();
        container.append(&port_label);

        // Browser button (hidden until URL is detected)
        let browser_button = gtk4::Button::builder()
            .icon_name("external-link-symbolic")
            .tooltip_text("Open in Browser")
            .css_classes(["flat", "status-chip", "browser-btn"])
            .visible(false)
            .build();
        container.append(&browser_button);

        let url: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

        // Wire browser button to open URL
        let url_ref = url.clone();
        browser_button.connect_clicked(move |btn| {
            if let Some(ref url_str) = *url_ref.borrow() {
                let launcher = gtk4::UriLauncher::new(url_str);
                let window = btn.root().and_then(|r| r.downcast::<gtk4::Window>().ok());
                launcher.launch(window.as_ref(), gio::Cancellable::NONE, |_| {});
            }
        });

        // Right-end cluster: the action buttons and the Ctrl+N keybind hint
        // share the same slot via an Overlay so neither reserves separate width
        // (which would squeeze the name) and the buttons don't shift on hover.
        // Both the button box and the hint are right-aligned so they sit in the
        // exact same place — at rest the hint shows, on hover (CSS) the hint
        // fades out and the buttons fade in.
        let actions_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        actions_box.set_halign(gtk4::Align::End);

        let play_button = gtk4::Button::builder()
            .icon_name("media-playback-start-symbolic")
            .tooltip_text(command)
            .css_classes(["flat", "status-chip", "process-play-btn", "btn-play"])
            .build();
        actions_box.append(&play_button);

        let restart_button = gtk4::Button::builder()
            .icon_name("view-refresh-symbolic")
            .tooltip_text("Restart")
            .css_classes(["flat", "status-chip", "process-play-btn"])
            .build();
        actions_box.append(&restart_button);

        let stop_button = gtk4::Button::builder()
            .icon_name("media-playback-stop-symbolic")
            .tooltip_text("Stop")
            .css_classes(["flat", "status-chip", "process-play-btn", "btn-stop"])
            .build();
        actions_box.append(&stop_button);

        // Ctrl+N shortcut hint (shown for the first 9 running processes).
        let keybind_label = gtk4::Label::builder()
            .css_classes(["caption", "dim-label", "process-keybind"])
            .halign(gtk4::Align::End)
            .valign(gtk4::Align::Center)
            // Decorative overlay — must not intercept clicks to the action
            // buttons beneath it (CSS opacity:0 on hover hides it visually but
            // would otherwise still swallow pointer events).
            .can_target(false)
            .visible(false)
            .build();

        let actions_overlay = gtk4::Overlay::new();
        actions_overlay.set_halign(gtk4::Align::End);
        actions_overlay.set_child(Some(&actions_box));
        actions_overlay.add_overlay(&keybind_label);
        // Size the overlay to the action buttons only, so the keybind hint
        // never reserves extra width and squeezes the name label.
        actions_overlay.set_measure_overlay(&keybind_label, false);
        container.append(&actions_overlay);

        let on_context_action: ActionCallback = Rc::new(RefCell::new(None));
        let action_name: Rc<RefCell<String>> = Rc::new(RefCell::new(name.to_string()));

        // Wire play button to trigger "toggle" action
        let on_action_ref = on_context_action.clone();
        let aname = action_name.clone();
        play_button.connect_clicked(move |_| {
            if let Some(ref cb) = *on_action_ref.borrow() {
                cb(&aname.borrow(), "toggle");
            }
        });

        // Wire restart button
        let on_action_ref = on_context_action.clone();
        let aname = action_name.clone();
        restart_button.connect_clicked(move |_| {
            if let Some(ref cb) = *on_action_ref.borrow() {
                cb(&aname.borrow(), "restart");
            }
        });

        // Wire stop button
        let on_action_ref = on_context_action.clone();
        let aname = action_name.clone();
        stop_button.connect_clicked(move |_| {
            if let Some(ref cb) = *on_action_ref.borrow() {
                cb(&aname.borrow(), "stop");
            }
        });

        // The browser section is part of the menu model that `set_url`
        // mutates before any right-click happens; build it eagerly so the
        // model stays in sync, and reuse the same instance once the
        // PopoverMenu is materialised on first right-click.
        let browser_section = gio::Menu::new();

        // Right-click context menu — built lazily on first right-click.
        let popover_cell: Rc<RefCell<Option<gtk4::PopoverMenu>>> = Rc::new(RefCell::new(None));
        let gesture = gtk4::GestureClick::builder()
            .button(3) // right click
            .build();
        let popover_ref = popover_cell.clone();
        let container_for_popover = container.clone();
        let command_for_popover = command.to_string();
        let category_for_popover = category.clone();
        let on_action_for_popover = on_context_action.clone();
        let url_for_popover = url.clone();
        let action_name_for_popover = action_name.clone();
        let browser_section_for_popover = browser_section.clone();
        gesture.connect_released(move |_, _, x, y| {
            let mut slot = popover_ref.borrow_mut();
            let popover = slot.get_or_insert_with(|| {
                let p = Self::build_context_menu(
                    &command_for_popover,
                    category_for_popover.clone(),
                    &on_action_for_popover,
                    &url_for_popover,
                    &action_name_for_popover,
                    &browser_section_for_popover,
                );
                p.set_parent(&container_for_popover);
                p
            });
            popover.set_pointing_to(Some(&gtk4::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            popover.popup();
        });
        container.add_controller(gesture);

        // Initially show play, hide stop and restart (default state is Stopped)
        stop_button.set_visible(false);
        restart_button.set_visible(false);

        Self {
            container,
            status_dot,
            status_stack,
            status_area,
            anim,
            is_terminal,
            is_running: Cell::new(false),
            name_label,
            action_name,
            qualified_name: Rc::new(RefCell::new(String::new())),
            keybind_label,
            port_label,
            browser_button,
            play_button,
            restart_button,
            stop_button,
            on_context_action,
            url,
            browser_menu_section: browser_section,
        }
    }

    fn build_context_menu(
        command: &str,
        category: ProcessCategory,
        on_action: &ActionCallback,
        url: &Rc<RefCell<Option<String>>>,
        action_name: &Rc<RefCell<String>>,
        browser_section: &gio::Menu,
    ) -> gtk4::PopoverMenu {
        let menu = gio::Menu::new();

        let control_section = gio::Menu::new();
        control_section.append(Some("Start / Stop"), Some("proc.toggle"));
        control_section.append(Some("Restart"), Some("proc.restart"));
        let show_resume = category == ProcessCategory::Agent
            && crate::util::notifications::resume_command_for(command).is_some();
        if show_resume {
            control_section.append(Some("Resume Session"), Some("proc.resume"));
        }
        menu.append_section(None, &control_section);

        // Browser section is owned by ProcessRow so `set_url` can mutate it
        // before the popover exists. Attach it to the menu model here.
        menu.append_section(None, browser_section);

        let terminal_section = gio::Menu::new();
        terminal_section.append(Some("Edit Command"), Some("proc.edit"));
        terminal_section.append(Some("Clear Output"), Some("proc.clear"));
        terminal_section.append(Some("Redraw Terminal"), Some("proc.redraw"));
        terminal_section.append(Some("Copy Command"), Some("proc.copy_command"));
        menu.append_section(None, &terminal_section);

        let danger_section = gio::Menu::new();
        let delete_item = gio::MenuItem::new(None, None);
        delete_item.set_attribute_value("custom", Some(&"delete-button".to_variant()));
        danger_section.append_item(&delete_item);
        menu.append_section(None, &danger_section);

        let popover = gtk4::PopoverMenu::from_model(Some(&menu));
        popover.set_has_arrow(false);

        // Custom red delete button
        let delete_btn = gtk4::Button::builder()
            .label("Delete Command")
            .css_classes(["flat", "destructive-menu-item"])
            .build();
        popover.add_child(&delete_btn, "delete-button");

        let action_group = gio::SimpleActionGroup::new();

        // Helper to create context actions — reads from shared action_name
        let add_action = |action_name_str: &str, action_str: &str| {
            let on_action_ref = on_action.clone();
            let aname = action_name.clone();
            let action_owned = action_str.to_string();
            let action = gio::SimpleAction::new(action_name_str, None);
            action.connect_activate(move |_, _| {
                if let Some(ref cb) = *on_action_ref.borrow() {
                    cb(&aname.borrow(), &action_owned);
                }
            });
            action_group.add_action(&action);
        };

        add_action("toggle", "toggle");
        add_action("restart", "restart");
        if show_resume {
            add_action("resume", "resume");
        }
        add_action("edit", "edit");
        add_action("clear", "clear");
        add_action("redraw", "redraw");

        // Wire delete button directly (custom widget, not in action group)
        let on_action_ref = on_action.clone();
        let aname = action_name.clone();
        let popover_ref = popover.clone();
        delete_btn.connect_clicked(move |_| {
            popover_ref.popdown();
            if let Some(ref cb) = *on_action_ref.borrow() {
                cb(&aname.borrow(), "delete");
            }
        });

        // Copy command — uses clipboard directly
        let command_owned = command.to_string();
        let copy_action = gio::SimpleAction::new("copy_command", None);
        copy_action.connect_activate(move |_, _| {
            if let Some(display) = gtk4::gdk::Display::default() {
                display.clipboard().set_text(&command_owned);
            }
        });
        action_group.add_action(&copy_action);

        // Open in Browser action
        let url_ref = url.clone();
        let open_url_action = gio::SimpleAction::new("open_url", None);
        let popover_ref2 = popover.clone();
        open_url_action.connect_activate(move |_, _| {
            if let Some(ref url_str) = *url_ref.borrow() {
                let launcher = gtk4::UriLauncher::new(url_str);
                let window = popover_ref2
                    .root()
                    .and_then(|r| r.downcast::<gtk4::Window>().ok());
                launcher.launch(window.as_ref(), gio::Cancellable::NONE, |_| {});
            }
        });
        action_group.add_action(&open_url_action);

        popover.insert_action_group("proc", Some(&action_group));

        popover
    }

    /// Agent-only: while the agent is actively producing output the dot
    /// morphs into a spinning square (see WorkingAnim); morphs back to the
    /// steady dot when it goes idle. No-op unless running.
    pub fn set_working(&self, working: bool) {
        let want = working && self.is_running.get();
        if want == self.anim.active.get() {
            return;
        }
        log::debug!("agent status dot: working={want}");
        self.anim.active.set(want);
        if want {
            self.status_stack.set_visible_child_name("working");
            self.start_working_tick();
        }
        // On stop the tick callback morphs back to a circle, swaps the
        // stack to the dot, and unregisters itself.
    }

    /// Whether the working animation is currently active (or winding down).
    pub fn is_working(&self) -> bool {
        self.anim.active.get()
    }

    fn start_working_tick(&self) {
        if self.anim.ticking.replace(true) {
            return;
        }
        self.anim.last_us.set(i64::MIN);
        let anim = self.anim.clone();
        let stack = self.status_stack.downgrade();
        self.status_area.add_tick_callback(move |area, clock| {
            let now = clock.frame_time();
            let last = anim.last_us.replace(now);
            let dt = if last == i64::MIN {
                0.0
            } else {
                // Clamp so unmapped gaps (collapsed project) don't jump.
                ((now - last) as f64 / 1e6).clamp(0.0, 0.05)
            };

            // Morph toward square while active, back toward circle after.
            let target = if anim.active.get() { 1.0 } else { 0.0 };
            let step = dt / MORPH_SECS;
            let m = anim.morph.get();
            let m = if target > m {
                (m + step).min(1.0)
            } else {
                (m - step).max(0.0)
            };
            anim.morph.set(m);

            // Spin pace: rapid spin-up burst, then a long wind-down to a
            // crawl, repeating until the agent finishes. Scaled by the
            // morph so the circle phases don't spin invisibly.
            anim.phase.set(anim.phase.get() + dt);
            let p = (anim.phase.get() / SPIN_CYCLE_SECS).fract();
            let envelope = if p < SPIN_ATTACK {
                // Ease-out attack: jumps toward full speed immediately.
                let a = p / SPIN_ATTACK;
                a * (2.0 - a)
            } else {
                // Quadratic release: sheds most speed early, then lingers
                // at a crawl until the next burst.
                let q = (p - SPIN_ATTACK) / (1.0 - SPIN_ATTACK);
                (1.0 - q) * (1.0 - q)
            };
            let speed = SPIN_SLOW + (SPIN_FAST - SPIN_SLOW) * envelope;
            anim.angle.set(anim.angle.get() + speed * dt * m);

            area.queue_draw();

            // Fully morphed back: show the plain dot again and stop ticking.
            if !anim.active.get() && m <= 0.0 {
                anim.ticking.set(false);
                anim.angle.set(0.0);
                anim.phase.set(0.0);
                if let Some(stack) = stack.upgrade() {
                    stack.set_visible_child_name("dot");
                }
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
    }

    pub fn set_status(&self, status: ProcessStatus) {
        // Remove old CSS classes from dot
        self.status_dot.remove_css_class("status-running");
        self.status_dot.remove_css_class("status-stopped");
        self.status_dot.remove_css_class("status-crashed");
        self.status_dot.remove_css_class("status-restarting");
        // Wind down the working animation if a status change interrupts it.
        self.anim.active.set(false);

        let is_running = matches!(status, ProcessStatus::Running | ProcessStatus::Restarting);
        self.play_button.set_visible(!is_running);
        self.stop_button.set_visible(is_running);
        self.restart_button.set_visible(is_running);

        match status {
            ProcessStatus::Running | ProcessStatus::Restarting => {
                self.is_running.set(true);
                self.status_dot.add_css_class("status-running");
            }
            ProcessStatus::Stopped => {
                self.is_running.set(false);
                self.status_dot.add_css_class("status-stopped");
                self.set_port(None);
                self.set_url(None);
            }
            ProcessStatus::Crashed => {
                self.is_running.set(false);
                self.status_dot.add_css_class("status-crashed");
                self.set_port(None);
                self.set_url(None);
            }
        }
    }

    pub fn set_port(&self, port: Option<u16>) {
        match port {
            Some(p) => {
                self.port_label.set_label(&format!(":{p}"));
                self.port_label.set_visible(true);
            }
            None => {
                if !self.port_label.is_visible() {
                    return;
                }
                self.port_label.set_visible(false);
            }
        }
    }

    /// Show the "Ctrl+N" shortcut hint, or hide it when `n` is `None`.
    pub fn set_keybind(&self, n: Option<usize>) {
        match n {
            Some(n) => {
                self.keybind_label.set_label(&format!("Ctrl+{n}"));
                self.keybind_label.set_visible(true);
            }
            None => self.keybind_label.set_visible(false),
        }
    }

    pub fn set_url(&self, url: Option<&str>) {
        match url {
            Some(u) => {
                if self.url.borrow().as_deref() == Some(u) {
                    return;
                }
                *self.url.borrow_mut() = Some(u.to_string());
                self.browser_button.set_visible(true);
                self.browser_button
                    .set_tooltip_text(Some(&format!("Open {u}")));
                if self.browser_menu_section.n_items() == 0 {
                    self.browser_menu_section
                        .append(Some("Open in Browser"), Some("proc.open_url"));
                }
            }
            None => {
                if self.url.borrow().is_none() {
                    return;
                }
                *self.url.borrow_mut() = None;
                self.browser_button.set_visible(false);
                self.browser_menu_section.remove_all();
            }
        }
    }

    pub fn get_url(&self) -> Option<String> {
        self.url.borrow().clone()
    }

    pub fn widget(&self) -> &gtk4::Box {
        &self.container
    }

    pub fn set_on_context_action(&self, cb: impl Fn(&str, &str) + 'static) {
        *self.on_context_action.borrow_mut() = Some(Box::new(cb));
    }

    pub fn name(&self) -> String {
        self.name_label.label().to_string()
    }

    pub fn set_name(&self, name: &str) {
        self.name_label.set_label(name);
    }

    /// Update the internal process name used by button/menu actions.
    /// Call this when the process is renamed (not for display_name changes).
    pub fn set_action_name(&self, name: &str) {
        *self.action_name.borrow_mut() = name.to_string();
    }

    pub fn set_command_tooltip(&self, command: &str) {
        self.container.set_tooltip_text(Some(command));
    }
}

/// Path a square of half-side `half` centered on the origin with corner
/// radius `radius`. radius == half yields a circle (the morph endpoints).
fn rounded_square(cr: &cairo::Context, half: f64, radius: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    let c = half - radius;
    cr.new_sub_path();
    cr.arc(c, -c, radius, -FRAC_PI_2, 0.0); // top-right
    cr.arc(c, c, radius, 0.0, FRAC_PI_2); // bottom-right
    cr.arc(-c, c, radius, FRAC_PI_2, PI); // bottom-left
    cr.arc(-c, -c, radius, PI, 1.5 * PI); // top-left
    cr.close_path();
}
