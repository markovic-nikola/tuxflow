//! The shared Adwaita-flavoured form furniture: a titled card of rows, and
//! the rows that go in it.
//!
//! These started life private to `settings_ui`, which is why they look like
//! `AdwPreferencesGroup`/`AdwActionRow`. The add-project flow renders the
//! same shapes (a "Project Name" entry row, a switch per detected command),
//! so they live here generic over the message type rather than being copied
//! into a second module and drifting from it.

use iced::widget::{column, container, row, text, toggler};
use iced::{Element, Length};

use crate::theme::{self, DIM, TEXT, TEXT_SECONDARY, bold};

/// An `AdwPreferencesGroup`: an optional title over a card of rows.
/// Rows carry their own padding and butt against each other, as Adwaita's do.
pub fn group<'a, M: 'a>(title: &'a str, rows: Vec<Element<'a, M>>) -> Element<'a, M> {
    let mut inner = column![].spacing(0);
    for r in rows {
        inner = inner.push(r);
    }
    let mut outer = column![].spacing(8);
    if !title.is_empty() {
        outer = outer.push(text(title).size(11.5).font(bold()).color(TEXT_SECONDARY));
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
