//! Add SSH Connection — GTK's `add_ssh_dialog` as a full-pane form on the
//! add-command idiom: the view and the pure state live here, the handlers
//! that touch the workspace stay in `App` (`update_add_ssh`).
//!
//! GTK's dialog carries a Project combo because its only entry is the
//! global palette; here the form is raised ON a project (header menu, or
//! the palette on the active card) like the other creators, so the combo
//! has no job — `project` names the card it was raised on.

use iced::Element;
use iced::widget::{button, checkbox, column, pick_list, row, text, text_input};
use tuxflow_core::config::ssh::{SshConnectionFields, SshHost};

use crate::theme::{self, CRASHED, bold, pal};
use crate::widgets::form_card;

/// The picker's first row: no alias, fields start blank (GTK's label).
pub const CUSTOM_HOST: &str = "Custom...";

pub struct State {
    /// The project the connection is added to.
    pub project: u64,
    pub hosts: Vec<SshHost>,
    /// The picker's current row — `CUSTOM_HOST` or an alias in `hosts`.
    pub host_choice: String,
    pub fields: SshConnectionFields,
    pub auto_connect: bool,
    pub auto_reconnect: bool,
    /// Why the last submit was refused; cleared on the next field edit.
    pub error: Option<String>,
}

impl State {
    pub fn new(project: u64, hosts: Vec<SshHost>) -> Self {
        Self {
            project,
            hosts,
            host_choice: CUSTOM_HOST.to_string(),
            fields: SshConnectionFields::default(),
            auto_connect: false,
            auto_reconnect: false,
            error: None,
        }
    }

    /// GTK's `connect_selected_notify`: an alias fills every field from
    /// the config entry, "Custom..." clears them back to the defaults.
    pub fn pick_host(&mut self, choice: String) {
        self.fields = match self.hosts.iter().find(|h| h.name == choice) {
            Some(host) => SshConnectionFields::from_host(host),
            None => SshConnectionFields::default(),
        };
        self.host_choice = choice;
        self.error = None;
    }

    /// Pure field edits; `Submit`/`Cancel` are the app's.
    pub fn edit(&mut self, msg: &Msg) {
        match msg {
            Msg::HostChoice(choice) => self.pick_host(choice.clone()),
            Msg::Name(v) => self.fields.name = v.clone(),
            Msg::Host(v) => self.fields.host = v.clone(),
            Msg::User(v) => self.fields.user = v.clone(),
            Msg::Port(v) => self.fields.port = v.clone(),
            Msg::Identity(v) => self.fields.identity_file = v.clone(),
            Msg::ToggleAutoConnect(v) => self.auto_connect = *v,
            Msg::ToggleAutoReconnect(v) => self.auto_reconnect = *v,
            Msg::Submit | Msg::Cancel => return,
        }
        self.error = None;
    }
}

#[derive(Debug, Clone)]
pub enum Msg {
    HostChoice(String),
    Name(String),
    Host(String),
    User(String),
    Port(String),
    Identity(String),
    ToggleAutoConnect(bool),
    ToggleAutoReconnect(bool),
    Submit,
    Cancel,
}

pub fn view(state: &'_ State, accent: iced::Color) -> Element<'_, Msg> {
    let caption = |label: &'static str| {
        text(label)
            .size(11.5)
            .font(bold())
            .color(pal().text_secondary)
    };
    let field = |label: &'static str,
                 placeholder: &'static str,
                 value: &str,
                 on_input: fn(String) -> Msg|
     -> Element<'_, Msg> {
        column![
            caption(label),
            text_input(placeholder, value)
                .on_input(on_input)
                .on_submit(Msg::Submit)
                .style(theme::input(accent))
                .padding([8, 14])
                .size(13),
        ]
        .spacing(6)
        .into()
    };

    let mut options: Vec<String> = vec![CUSTOM_HOST.to_string()];
    options.extend(state.hosts.iter().map(|h| h.name.clone()));
    let picker = crate::widgets::row_base(
        "SSH Config Host",
        "Pick from ~/.ssh/config or enter custom",
        pick_list(options, Some(state.host_choice.clone()), Msg::HostChoice)
            .text_size(12)
            .padding([4, 10])
            .into(),
    );

    let mut col = column![text("New SSH Connection").size(16).font(bold()), picker]
        .spacing(14)
        .push(field(
            "Name",
            "Optional, defaults to user@host",
            &state.fields.name,
            Msg::Name,
        ))
        .push(field(
            "Host (required)",
            "e.g. 10.0.1.50",
            &state.fields.host,
            Msg::Host,
        ))
        .push(field("User", "Optional", &state.fields.user, Msg::User))
        .push(field("Port", "22", &state.fields.port, Msg::Port))
        .push(field(
            "Identity file (optional)",
            "e.g. ~/.ssh/id_ed25519",
            &state.fields.identity_file,
            Msg::Identity,
        ))
        .push(
            checkbox(state.auto_connect)
                .label("Auto-connect when the project starts")
                .on_toggle(Msg::ToggleAutoConnect)
                .size(16)
                .text_size(12.5),
        )
        .push(
            checkbox(state.auto_reconnect)
                .label("Auto-reconnect if the connection drops")
                .on_toggle(Msg::ToggleAutoReconnect)
                .size(16)
                .text_size(12.5),
        );
    if let Some(error) = &state.error {
        col = col.push(text(error).size(12).color(CRASHED));
    }
    // GTK's Add button is insensitive until the host is filled in; the
    // primary style's Disabled arm paints the same thing here.
    let mut add = button(text("Add").size(12).font(bold()))
        .padding([7, 16])
        .style(theme::primary(accent));
    if state.fields.host_given() {
        add = add.on_press(Msg::Submit);
    }
    col = col.push(
        row![
            add,
            button(text("Cancel").size(12))
                .padding([7, 16])
                .style(theme::pill_button(accent))
                .on_press(Msg::Cancel),
        ]
        .spacing(8),
    );
    form_card(col)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosts() -> Vec<SshHost> {
        vec![SshHost {
            name: "dev".into(),
            hostname: Some("10.0.1.50".into()),
            user: Some("devuser".into()),
            port: Some(2222),
            identity_file: Some("~/.ssh/dev".into()),
        }]
    }

    #[test]
    fn picking_an_alias_fills_and_custom_clears() {
        let mut s = State::new(1, hosts());
        s.fields.name = "typed".into();
        s.edit(&Msg::HostChoice("dev".into()));
        assert_eq!(s.fields.host, "10.0.1.50");
        assert_eq!(s.fields.user, "devuser");
        assert_eq!(s.fields.port, "2222");
        assert_eq!(s.fields.name, "dev");
        assert_eq!(s.host_choice, "dev");
        s.edit(&Msg::HostChoice(CUSTOM_HOST.into()));
        assert_eq!(s.fields, SshConnectionFields::default());
        assert_eq!(s.host_choice, CUSTOM_HOST);
    }

    #[test]
    fn edits_clear_the_error_and_toggles_stick() {
        let mut s = State::new(1, hosts());
        s.error = Some("x".into());
        s.edit(&Msg::Host("h".into()));
        assert!(s.error.is_none());
        assert!(s.fields.host_given());
        s.edit(&Msg::ToggleAutoConnect(true));
        s.edit(&Msg::ToggleAutoReconnect(true));
        assert!(s.auto_connect && s.auto_reconnect);
    }
}
