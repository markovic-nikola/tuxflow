//! Vite dev-server discovery for remote projects, for the ports terminal
//! output never mentions.
//!
//! `PortDetector` learns ports by reading what a process prints, which holds
//! for `composer dev` (concurrently interleaves its children's lines) but not
//! for Laravel 13's `php artisan dev`: that runs `@laravel/multiplex`, a tabbed
//! TUI which only ever draws the *selected* tab. Vite's
//! `➜  Local: http://127.0.0.1:5173/` banner stays in multiplex's own buffer
//! and never reaches the terminal, so on a remote project the asset port goes
//! untunnelled and the page loads with every `@vite` request refused.
//!
//! Both runners ship in the same app — a project can have a concurrently-based
//! `dev` script in composer.json *and* the artisan command — so this probe is
//! unconditional rather than keyed to a detected runner. It reads the port out
//! of the same file the browser is about to be pointed at, which makes it
//! authoritative where output scanning is merely lucky, and a no-op when
//! scanning already found the port.

use crate::remote::fs::ProjectFs;

/// Where laravel-vite-plugin publishes the dev-server URL while it is serving.
/// Its `hotFile` option defaults to `<publicDirectory>/hot`; the plugin writes
/// the file on listen and removes it on graceful shutdown (so a killed Vite
/// leaves one behind, and the app keeps serving dead asset URLs until the next
/// run overwrites it).
const HOT_FILE: &str = "public/hot";

/// The port Vite is serving on, per the project's hot file, or None when it
/// isn't serving / the file can't be read.
///
/// Blocking: on a remote project this is an ssh round trip. Call it from a
/// worker thread, never the GTK main loop.
pub fn hot_port(fs: &dyn ProjectFs) -> Option<u16> {
    let raw = fs.read_to_string(HOT_FILE).ok()?;
    parse_hot(&raw)
}

/// The hot file holds the dev-server URL with Vite's base path appended
/// (`${viteDevServerUrl}${base}`), so `http://127.0.0.1:5173/build` is as
/// normal as a bare origin — parse the authority, not the whole string.
fn parse_hot(raw: &str) -> Option<u16> {
    let url = raw.trim();
    // A hot file written for a unix socket has no port to forward.
    crate::util::port_detector::extract_host_port_from_url(url).map(|(_, port)| port)
}

#[cfg(test)]
mod tests {
    use super::parse_hot;

    #[test]
    fn parses_bare_origin() {
        assert_eq!(parse_hot("http://127.0.0.1:5173"), Some(5173));
    }

    #[test]
    fn parses_origin_with_vite_base_path() {
        // laravel-vite-plugin appends `server.config.base`
        assert_eq!(parse_hot("http://127.0.0.1:5173/build"), Some(5173));
    }

    #[test]
    fn parses_localhost_host_form() {
        assert_eq!(parse_hot("http://localhost:5174"), Some(5174));
    }

    #[test]
    fn tolerates_trailing_newline() {
        assert_eq!(parse_hot("http://127.0.0.1:5173\n"), Some(5173));
    }

    #[test]
    fn https_dev_server() {
        assert_eq!(parse_hot("https://127.0.0.1:5173"), Some(5173));
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_hot(""), None);
        assert_eq!(parse_hot("not a url"), None);
    }
}
