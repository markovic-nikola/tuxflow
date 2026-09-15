//! The desktop's light/dark preference, for Settings → Appearance →
//! Color Scheme = System. libadwaita reads it for the GTK shell through
//! the same door: the settings portal's `org.freedesktop.appearance`
//! `color-scheme` key (1 = prefer dark, 2 = prefer light, 0 = no
//! preference), which GNOME, KDE, Cinnamon and the rest all serve. One
//! blocking read at launch, then the portal's `SettingChanged` signal for
//! the rest of the session — a daytime/nighttime switch on the desktop
//! should flip the shell without a restart, as it flips every GTK app.

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedValue, Value};

const NAMESPACE: &str = "org.freedesktop.appearance";
const KEY: &str = "color-scheme";

fn portal(conn: &Connection) -> zbus::Result<Proxy<'static>> {
    Proxy::new(
        conn,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Settings",
    )
}

/// The portal's value → "prefers light". 0 (no preference) and unknown
/// values read as dark, the shell's native scheme.
fn prefers_light(value: &Value<'_>) -> Option<bool> {
    let inner = match value {
        Value::Value(v) => v.as_ref(),
        other => other,
    };
    match inner {
        Value::U32(n) => Some(*n == 2),
        _ => None,
    }
}

/// Ask the portal once. `None` when there is no portal to ask (no session
/// bus, no xdg-desktop-portal) — the caller falls back to dark.
pub fn system_prefers_light() -> Option<bool> {
    let conn = Connection::session().ok()?;
    let proxy = portal(&conn).ok()?;
    let value: OwnedValue = proxy.call("ReadOne", &(NAMESPACE, KEY)).ok()?;
    prefers_light(&value)
}

/// Follow the portal's `SettingChanged` for the color-scheme key, calling
/// `on_change` from a dedicated thread with each new preference. Quietly
/// does nothing when the portal is not there.
pub fn watch(on_change: impl Fn(bool) + Send + 'static) {
    std::thread::spawn(move || {
        let Ok(conn) = Connection::session() else {
            return;
        };
        let Ok(proxy) = portal(&conn) else {
            return;
        };
        let Ok(signals) = proxy.receive_signal("SettingChanged") else {
            return;
        };
        for message in signals {
            let body = message.body();
            let Ok((namespace, key, value)) = body.deserialize::<(String, String, OwnedValue)>()
            else {
                continue;
            };
            if namespace == NAMESPACE
                && key == KEY
                && let Some(light) = prefers_light(&value)
            {
                on_change(light);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portal_values_map_to_light_or_dark() {
        assert_eq!(prefers_light(&Value::U32(2)), Some(true));
        assert_eq!(prefers_light(&Value::U32(1)), Some(false));
        assert_eq!(prefers_light(&Value::U32(0)), Some(false));
        // ReadOne wraps the setting in a variant.
        assert_eq!(
            prefers_light(&Value::Value(Box::new(Value::U32(2)))),
            Some(true)
        );
        assert_eq!(prefers_light(&Value::Str("light".into())), None);
    }
}
