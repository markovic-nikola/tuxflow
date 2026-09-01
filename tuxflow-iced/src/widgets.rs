//! The shared Adwaita-flavoured form furniture: a titled card of rows, and
//! the rows that go in it.
//!
//! These started life private to `settings_ui`, which is why they look like
//! `AdwPreferencesGroup`/`AdwActionRow`. The add-project flow renders the
//! same shapes (a "Project Name" entry row, a switch per detected command),
//! so they live here generic over the message type rather than being copied
//! into a second module and drifting from it.

use std::path::Path;

use iced::widget::{button, column, container, row, scrollable, text, toggler};
use iced::{Element, Length};

use crate::theme::{self, DIM, TEXT, TEXT_SECONDARY, bold};

/// An `AdwPreferencesGroup`: an optional title over a card of rows.
/// Rows carry their own padding and butt against each other, as Adwaita's do.
pub fn group<'a, M: 'a>(title: &'a str, rows: Vec<Element<'a, M>>) -> Element<'a, M> {
    let header = (!title.is_empty()).then(|| {
        text(title)
            .size(11.5)
            .font(bold())
            .color(TEXT_SECONDARY)
            .into()
    });
    grouped(header, rows)
}

/// [`group`] over an owned title with `AdwPreferencesGroup`'s description
/// caption — the Edit Project command groups, whose titles carry live
/// counts and whose captions explain what a toggle does.
pub fn group_described<'a, M: 'a>(
    title: String,
    description: &'a str,
    rows: Vec<Element<'a, M>>,
) -> Element<'a, M> {
    let mut header = column![text(title).size(11.5).font(bold()).color(TEXT_SECONDARY)].spacing(3);
    if !description.is_empty() {
        header = header.push(text(description).size(10.5).color(DIM));
    }
    grouped(Some(header.into()), rows)
}

fn grouped<'a, M: 'a>(header: Option<Element<'a, M>>, rows: Vec<Element<'a, M>>) -> Element<'a, M> {
    let mut inner = column![].spacing(0);
    for r in rows {
        inner = inner.push(r);
    }
    let mut outer = column![].spacing(8);
    if let Some(h) = header {
        outer = outer.push(h);
    }
    outer
        .push(
            container(inner)
                .padding([6, 0])
                .style(theme::settings_card)
                .width(Length::Fill),
        )
        .into()
}

/// An `AdwActionRow`: title over an optional subtitle on the left, a control
/// on the right. Everything else in this module is a shape of this.
pub fn row_base<'a, M: 'a>(
    title: &'a str,
    subtitle: &'a str,
    control: Element<'a, M>,
) -> Element<'a, M> {
    let mut left = column![text(title).size(13).color(TEXT)].spacing(3);
    if !subtitle.is_empty() {
        left = left.push(text(subtitle).size(10.5).color(DIM));
    }
    container(
        row![left.width(Length::Fill), control]
            .spacing(12)
            .align_y(iced::Alignment::Center),
    )
    .padding([7, 14])
    .into()
}

/// Same as [`row_base`] but with an owned subtitle, for the ones built per
/// frame from state (a detected command's command line, a host's address).
pub fn row_owned<'a, M: 'a>(
    title: String,
    subtitle: String,
    control: Element<'a, M>,
) -> Element<'a, M> {
    let mut left = column![text(title).size(13).color(TEXT)].spacing(3);
    if !subtitle.is_empty() {
        left = left.push(text(subtitle).size(10.5).color(DIM));
    }
    container(
        row![left.width(Length::Fill), control]
            .spacing(12)
            .align_y(iced::Alignment::Center),
    )
    .padding([7, 14])
    .into()
}

/// A row that only states something.
pub fn label_row<'a, M: 'a>(title: &'a str, subtitle: &'a str) -> Element<'a, M> {
    row_base(title, subtitle, column![].into())
}

/// An `AdwSwitchRow`. Settings rows belong to no project, so they take the
/// local accent; anything project-scoped uses [`switch_row_owned`] and passes
/// the accent that matches where the project lives.
pub fn switch_row<'a, M: 'a>(
    title: &'a str,
    subtitle: &'a str,
    active: bool,
    f: impl Fn(bool) -> M + 'a,
) -> Element<'a, M> {
    row_base(title, subtitle, switch(active, theme::accent_for(false), f))
}

/// [`switch_row`] over owned strings — one per detected command.
pub fn switch_row_owned<'a, M: 'a>(
    title: String,
    subtitle: String,
    active: bool,
    accent: iced::Color,
    f: impl Fn(bool) -> M + 'a,
) -> Element<'a, M> {
    row_owned(title, subtitle, switch(active, accent, f))
}

fn switch<'a, M: 'a>(
    active: bool,
    accent: iced::Color,
    f: impl Fn(bool) -> M + 'a,
) -> Element<'a, M> {
    toggler(active)
        .on_toggle(f)
        .size(18)
        .style(theme::toggler(accent))
        .into()
}

/// Vector art goes to the `svg` widget, raster to `image` — iced has one
/// decoder each and no sniffing between them.
fn is_svg(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("svg"))
}

/// A project's avatar at `px`: its own artwork, or an initials square in
/// the project's accent. One drawing for the sidebar card and the Edit
/// Project preview, so the two can't disagree about what a saved icon
/// looks like.
pub fn avatar<'a, M: 'a>(
    icon: Option<&Path>,
    name: &str,
    accent: iced::Color,
    remote: bool,
    px: f32,
) -> Element<'a, M> {
    match icon {
        Some(path) => {
            // Drawn bare, as GTK does: `.project-icon-area` carries only
            // the rounded clip and the accent wash belongs to
            // `.project-icon`, the initials label. A logo sitting on an
            // accent tint reads as a *tinted logo*.
            let art: Element<'a, M> = if is_svg(path) {
                // No `svg::Style` here — that is the symbolic recolor and
                // these are full-colour art. No radius either: `svg` has
                // no `border_radius`, and vector logos bring their own
                // transparent corners, so GTK's clip is a no-op on them.
                iced::widget::svg(iced::widget::svg::Handle::from_path(path))
                    .width(px)
                    .height(px)
                    .into()
            } else {
                // The radius is GTK's `overflow: hidden`, and it earns its
                // place: a favicon with opaque corners is a hard square in
                // a column of rounded ones. iced applies it to the FITTED
                // art bounds rather than the layout box, so a non-square
                // logo gets its own corners rounded, not a crop.
                iced::widget::image(iced::widget::image::Handle::from_path(path))
                    .width(px)
                    .height(px)
                    .border_radius(8)
                    .into()
            };
            container(art).center_x(px).center_y(px).into()
        }
        None => {
            let initials: String = name
                .chars()
                .filter(|c| c.is_alphanumeric())
                .take(2)
                .collect::<String>()
                .to_uppercase();
            // 9pt on the sidebar's 26px box; larger boxes scale with it.
            container(text(initials).size(px * (9.0 / 26.0)).font(bold()))
                .center_x(px)
                .center_y(px)
                .style(theme::icon_square(accent, remote))
                .into()
        }
    }
}

/// The path-completion dropdown shared by the add-project and edit-project
/// forms: menu-item rows in a card, scrolling past ~8 entries. Callers add
/// it only when non-empty, so an empty box never sits under a field.
pub fn suggestion_list<'a, M: Clone + 'a>(
    paths: &'a [String],
    pick: impl Fn(String) -> M + 'a,
) -> Element<'a, M> {
    let mut rows = column![].spacing(0);
    for path in paths {
        rows = rows.push(
            button(text(shorten_path(path)).size(12).color(TEXT))
                .width(Length::Fill)
                .padding([6, 12])
                .style(theme::menu_item(false))
                .on_press(pick(path.clone())),
        );
    }
    container(
        scrollable(rows)
            .height(Length::Shrink)
            .direction(scrollable::Direction::Vertical(
                scrollable::Scrollbar::new().width(4).scroller_width(4),
            ))
            .style(theme::overlay_scrollbar),
    )
    .max_height(190)
    .padding([4, 0])
    .style(theme::settings_card)
    .width(Length::Fill)
    .into()
}

/// Suggestions are absolute paths and the tail is the part being completed,
/// so a long one drops its middle rather than its end (GTK ellipsizes at the
/// START for the same reason).
fn shorten_path(path: &str) -> String {
    const MAX: usize = 58;
    let chars: Vec<char> = path.chars().collect();
    if chars.len() <= MAX {
        return path.to_string();
    }
    let tail: String = chars[chars.len() - (MAX - 1)..].iter().collect();
    format!("\u{2026}{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_paths_keep_their_tail() {
        let long = format!("/home/nikola/{}/target", "deep".repeat(30));
        let short = shorten_path(&long);
        assert!(short.starts_with('\u{2026}'));
        assert!(short.ends_with("/target"), "{short}");
        assert!(short.chars().count() <= 58);
        assert_eq!(shorten_path("/srv/app"), "/srv/app");
    }
}
