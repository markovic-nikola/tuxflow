//! Opening a project in the user's configured editor, local or remote.

use crate::config::settings::AppSettings;
use crate::remote::ProjectLocation;

/// Editors speaking the VS Code CLI (`--reuse-window`, `--remote`).
const CODE_FAMILY: [&str; 4] = ["code", "cursor", "codium", "code-insiders"];

fn is_code_family(editor: &str) -> bool {
    CODE_FAMILY.contains(&editor)
}

fn on_path(binary: &str) -> bool {
    std::process::Command::new("which")
        .arg(binary)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Open `location` in the configured editor.
///
/// Local: spawn the editor on the directory; the `xdg-open` placeholder
/// default auto-picks an installed editor first. Remote: only code-family
/// editors can open a directory over SSH (`--remote ssh-remote+host`) — used
/// when configured, otherwise the first one found on PATH; without one the
/// action is a no-op with a warning log.
pub fn open_in_editor(location: &ProjectLocation) {
    let settings = AppSettings::load();
    let editor = settings.tools.default_editor.clone();

    match location {
        ProjectLocation::Local(dir) => {
            let resolved = if editor == "xdg-open" {
                [
                    "code",
                    "codium",
                    "zed",
                    "gnome-text-editor",
                    "gedit",
                    "kate",
                ]
                .into_iter()
                .find(|c| on_path(c))
                .unwrap_or("xdg-open")
                .to_string()
            } else {
                editor
            };
            let mut cmd = std::process::Command::new(&resolved);
            if settings.tools.reuse_editor_window && is_code_family(&resolved) {
                cmd.arg("--reuse-window");
            }
            cmd.arg(dir);
            if let Err(e) = cmd.spawn() {
                log::error!("Failed to open editor '{resolved}': {e}");
            }
        }
        ProjectLocation::Ssh { host, dir } => {
            let resolved = if is_code_family(&editor) {
                Some(editor)
            } else {
                CODE_FAMILY
                    .into_iter()
                    .find(|c| on_path(c))
                    .map(String::from)
            };
            let Some(resolved) = resolved else {
                log::warn!(
                    "No editor with SSH remote support found (need one of {CODE_FAMILY:?}) \
                     — can't open {host}:{dir}"
                );
                return;
            };
            let mut cmd = std::process::Command::new(&resolved);
            if settings.tools.reuse_editor_window {
                cmd.arg("--reuse-window");
            }
            cmd.args(["--remote", &format!("ssh-remote+{host}")])
                .arg(dir);
            if let Err(e) = cmd.spawn() {
                log::error!("Failed to open remote editor '{resolved}': {e}");
            }
        }
    }
}
