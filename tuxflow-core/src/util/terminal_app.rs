//! Opening a project in the user's configured terminal app (Settings →
//! Tools → Default Terminal), local or remote — the twin of [`super::editor`].
//!
//! Local projects get the terminal started in the directory (every
//! terminal inherits its cwd, so no per-app flag is needed). Remote ones
//! get an interactive ssh shell on the host, `cd`'d into the project — and
//! THAT does need per-app knowledge, since the flag that runs a command
//! inside a fresh window is one of `-e`, `-x`, `--` or nothing at all,
//! depending on the terminal. The table is the pure part
//! ([`terminal_command`]), so the spelling of each app's flag is pinned by
//! tests rather than discovered on the user's desktop.

use crate::config::settings::AppSettings;
use crate::remote::{ProjectLocation, sh_quote};

/// Tried in this order when the setting is the `xdg-open` placeholder
/// (there is no `xdg-open` for terminals; it means "whatever is here").
const CANDIDATES: [&str; 8] = [
    "gnome-terminal",
    "konsole",
    "xfce4-terminal",
    "alacritty",
    "kitty",
    "foot",
    "wezterm",
    "xterm",
];

fn on_path(binary: &str) -> bool {
    std::process::Command::new("which")
        .arg(binary)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// How `terminal` runs a command in a new window: the arguments that go
/// BEFORE the command line; empty means positional (the command follows
/// the program directly).
fn exec_prefix(terminal: &str) -> &'static [&'static str] {
    match terminal {
        "gnome-terminal" => &["--"],
        "wezterm" => &["start", "--"],
        "xfce4-terminal" | "mate-terminal" | "terminator" => &["-x"],
        "kitty" | "foot" => &[],
        // konsole, alacritty, ghostty, tilix, xterm and the rest of the
        // xterm-flag family.
        _ => &["-e"],
    }
}

/// The program and arguments that open `location` in `terminal`.
/// Local: the bare terminal (the caller sets its cwd). Remote: the
/// terminal running `ssh -t host 'cd dir && exec $SHELL -l'`.
pub fn terminal_command(terminal: &str, location: &ProjectLocation) -> (String, Vec<String>) {
    match location {
        ProjectLocation::Local(_) => (terminal.to_string(), Vec::new()),
        ProjectLocation::Ssh { host, dir } => {
            let remote = format!("cd {} && exec \"${{SHELL:-/bin/sh}}\" -l", sh_quote(dir));
            let mut args: Vec<String> = exec_prefix(terminal)
                .iter()
                .map(|s| s.to_string())
                .collect();
            args.extend(["ssh".to_string(), "-t".to_string(), host.clone(), remote]);
            (terminal.to_string(), args)
        }
    }
}

/// Resolve the configured terminal — the placeholder means the first
/// candidate on PATH.
fn resolve(configured: &str) -> Option<String> {
    if configured != "xdg-open" {
        return Some(configured.to_string());
    }
    CANDIDATES
        .into_iter()
        .find(|c| on_path(c))
        .map(String::from)
}

/// Open `location` in the configured terminal app. A no-op with a warning
/// when no terminal can be found.
pub fn open_terminal(location: &ProjectLocation) {
    let settings = AppSettings::load();
    let Some(terminal) = resolve(&settings.tools.default_terminal) else {
        log::warn!("No terminal app found (tried {CANDIDATES:?}) — can't open {location:?}");
        return;
    };
    let (program, args) = terminal_command(&terminal, location);
    let mut cmd = std::process::Command::new(&program);
    cmd.args(&args);
    if let ProjectLocation::Local(dir) = location {
        cmd.current_dir(dir);
    }
    if let Err(e) = cmd.spawn() {
        log::error!("Failed to open terminal '{program}': {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote() -> ProjectLocation {
        ProjectLocation::Ssh {
            host: "vps".into(),
            dir: "/home/deployer/Projects/it's here".into(),
        }
    }

    #[test]
    fn local_is_the_bare_terminal() {
        let (program, args) = terminal_command("kitty", &ProjectLocation::Local("/srv/app".into()));
        assert_eq!(program, "kitty");
        assert!(args.is_empty(), "cwd is set on the process, not passed");
    }

    #[test]
    fn remote_runs_an_ssh_shell_with_each_apps_exec_flag() {
        let shell = "cd '/home/deployer/Projects/it'\\''s here' && exec \"${SHELL:-/bin/sh}\" -l";
        let ssh = ["ssh", "-t", "vps", shell];
        let expect = |terminal: &str, prefix: &[&str]| {
            let (_, args) = terminal_command(terminal, &remote());
            let want: Vec<&str> = prefix.iter().chain(ssh.iter()).copied().collect();
            assert_eq!(args, want, "{terminal}");
        };
        expect("gnome-terminal", &["--"]);
        expect("wezterm", &["start", "--"]);
        expect("xfce4-terminal", &["-x"]);
        expect("kitty", &[]);
        expect("foot", &[]);
        expect("konsole", &["-e"]);
        expect("alacritty", &["-e"]);
        expect("ghostty", &["-e"]);
    }
}
