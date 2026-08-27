//! Project icon (avatar) detection and the policy for resolving one.
//!
//! Moved out of the GTK app during the iced migration so both shells scan
//! the same candidates in the same order — an icon that appears in one
//! window and not the other reads as a bug in whichever shell is newer.

use crate::config::projects::SavedProjects;
use std::path::Path;

/// Common icon file locations to check in a project directory, ordered by priority.
const CANDIDATES: &[&str] = &[
    // Explicit project icons
    "logo.svg",
    "logo.png",
    "logo.webp",
    "icon.svg",
    "icon.png",
    "icon.webp",
    // Web app assets
    "public/logo.svg",
    "public/logo.png",
    "public/logo.webp",
    "public/favicon.svg",
    "public/favicon.png",
    "public/favicon.webp",
    "public/favicon.ico",
    "public/img/favicon.svg",
    "public/img/favicon.png",
    "public/img/favicon.webp",
    "public/img/favicon.ico",
    "public/icon.svg",
    "public/icon.png",
    "public/icon.webp",
    "static/logo.svg",
    "static/logo.png",
    "static/logo.webp",
    "static/favicon.svg",
    "static/favicon.png",
    "static/favicon.webp",
    "static/favicon.ico",
    "assets/logo.svg",
    "assets/logo.png",
    "assets/logo.webp",
    "assets/icon.svg",
    "assets/icon.png",
    "assets/icon.webp",
    // `public/` and `static/` both list favicons but `assets/` did not, so a
    // project keeping its favicon there got no icon at all.
    "assets/favicon.svg",
    "assets/favicon.png",
    "assets/favicon.webp",
    "assets/favicon.ico",
    // Rust / Cargo
    "assets/icon.ico",
    // Electron / Tauri
    "src-tauri/icons/icon.png",
    "src-tauri/icons/icon.ico",
    "build/icon.png",
    // Freedesktop
    "data/icons/hicolor/scalable/apps/*.svg",
    "data/icons/hicolor/256x256/apps/*.png",
    // GitHub
    ".github/logo.svg",
    ".github/logo.png",
    ".github/icon.svg",
    ".github/icon.png",
];

/// Find project icons over a `ProjectFs` — the remote-capable variant.
/// Glob candidates are skipped (no remote glob support); everything else is
/// checked in one batched round trip. Returns every present, non-empty
/// match in priority order so callers can fall through when fetching one
/// fails.
pub fn detect_icons_fs(fs: &dyn crate::remote::fs::ProjectFs) -> Vec<&'static str> {
    let candidates: Vec<&'static str> = CANDIDATES
        .iter()
        .filter(|c| !c.contains('*'))
        .copied()
        .collect();
    let present = fs.exists_many_nonempty(&candidates);
    candidates
        .into_iter()
        .zip(present)
        .filter(|(_, p)| *p)
        .map(|(c, _)| c)
        .collect()
}

/// Non-empty regular file — 0-byte placeholders (Laravel's default
/// favicon.ico) must not win the scan.
fn is_usable_icon(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.len() > 0)
        .unwrap_or(false)
}

/// Try to find a project icon by checking common file locations.
/// Returns the absolute path to the first usable match found.
pub fn detect_icon(project_dir: &Path) -> Option<String> {
    for candidate in CANDIDATES {
        if candidate.contains('*') {
            if let Ok(matches) = glob::glob(&project_dir.join(candidate).to_string_lossy())
                && let Some(path) = matches.flatten().find(|p| is_usable_icon(p))
            {
                return Some(path.to_string_lossy().to_string());
            }
        } else {
            let path = project_dir.join(candidate);
            if is_usable_icon(&path) {
                return Some(path.to_string_lossy().to_string());
            }
        }
    }
    None
}

/// Resolve a project's icon at load: the saved path wins (it is the user's
/// pick in Edit Project, or an earlier detection), otherwise a local project
/// scans its own disk while a remote one takes `hint` — the file its probe
/// already fetched into `~/.cache/tuxflow/icons/`.
///
/// A fresh detection is remembered, so the scan runs once per project rather
/// than every launch. `set_icon` persists on its own — as every setter on
/// [`SavedProjects`] does — so there is no `save()` here; adding one would
/// write the file twice for a single change.
///
/// Blocking (stats the project dir); call off a UI thread for remote work.
pub fn resolve_icon(
    saved: &mut SavedProjects,
    key: &str,
    local_dir: Option<&Path>,
    hint: Option<String>,
) -> Option<String> {
    if let Some(path) = saved.get_icon(key) {
        return Some(path.clone());
    }
    // A local project scans its own disk; a remote one has no local dir and
    // falls through to the hint — the icon its probe already pulled into the
    // cache. The hint is only ever set for remote projects, but the order
    // makes the precedence explicit rather than incidental.
    let detected = local_dir.and_then(detect_icon).or(hint);
    if let Some(path) = &detected {
        log::info!("Auto-detected project icon: {path}");
        saved.set_icon(key, Some(path.clone()));
    }
    detected
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A favicon under `assets/` used to be invisible to detection even though
    /// the same name is checked under `public/` and `static/`.
    #[test]
    fn assets_favicon_is_a_candidate() {
        for name in [
            "assets/favicon.svg",
            "assets/favicon.png",
            "assets/favicon.webp",
            "assets/favicon.ico",
        ] {
            assert!(CANDIDATES.contains(&name), "{name} missing from CANDIDATES");
        }
    }

    #[test]
    fn detects_an_assets_favicon_on_disk() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        std::fs::create_dir_all(&assets).expect("assets dir");
        std::fs::write(assets.join("favicon.svg"), b"<svg/>").expect("write icon");

        assert_eq!(
            detect_icon(tmp.path()),
            Some(assets.join("favicon.svg").to_string_lossy().into_owned())
        );
    }

    /// A `SavedProjects` bound to a temp file — the only kind a test may
    /// mutate. Every setter persists, so an unbound one silently writes
    /// nothing and the real config is never a candidate target.
    fn scratch_saved(dir: &std::path::Path) -> SavedProjects {
        SavedProjects::load_from(dir.join("projects.toml"))
    }

    /// The detect-and-remember branch: a detected icon is both returned AND
    /// written back, so the scan runs once per project instead of every
    /// launch. This had no coverage until `load_from` made it reachable
    /// without writing the developer's real workspace.
    #[test]
    fn a_detected_icon_is_remembered() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let project = tmp.path().join("proj");
        std::fs::create_dir_all(&project).expect("project dir");
        std::fs::write(project.join("logo.svg"), b"<svg/>").expect("write icon");
        let expected = project.join("logo.svg").to_string_lossy().into_owned();

        let mut saved = scratch_saved(tmp.path());
        let resolved = resolve_icon(&mut saved, "k", Some(&project), None);
        assert_eq!(resolved.as_deref(), Some(expected.as_str()));

        // Reload from disk: the remembering has to survive the process, which
        // is the whole reason it happens at all.
        let reloaded = SavedProjects::load_from(tmp.path().join("projects.toml"));
        assert_eq!(reloaded.get_icon("k").map(String::as_str), Some(&*expected));
    }

    /// A saved icon — the user's pick in Edit Project — outranks whatever is
    /// on disk, and costs no scan. That short-circuit is what keeps a remote
    /// project's probe from opening an ssh round trip it doesn't need.
    #[test]
    fn a_saved_icon_wins_over_detection() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let project = tmp.path().join("proj");
        std::fs::create_dir_all(&project).expect("project dir");
        std::fs::write(project.join("logo.svg"), b"<svg/>").expect("write icon");

        let mut saved = scratch_saved(tmp.path());
        saved.set_icon("k", Some("/chosen/by-hand.png".into()));

        let resolved = resolve_icon(&mut saved, "k", Some(&project), None);
        assert_eq!(resolved.as_deref(), Some("/chosen/by-hand.png"));
    }

    /// A remote project has no local dir to scan, so it falls through to the
    /// hint — the icon its probe already pulled into the cache — and that is
    /// remembered like any other detection.
    #[test]
    fn a_remote_project_falls_back_to_the_probes_hint() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let hint = "/home/me/.cache/tuxflow/icons/abc.png";

        let mut saved = scratch_saved(tmp.path());
        let resolved = resolve_icon(&mut saved, "ssh://h/d", None, Some(hint.into()));

        assert_eq!(resolved.as_deref(), Some(hint));
        assert_eq!(saved.get_icon("ssh://h/d").map(String::as_str), Some(hint));
    }

    /// A local project prefers what is actually on its disk over a hint.
    #[test]
    fn a_local_icon_outranks_a_hint() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let project = tmp.path().join("proj");
        std::fs::create_dir_all(&project).expect("project dir");
        std::fs::write(project.join("logo.svg"), b"<svg/>").expect("write icon");

        let mut saved = scratch_saved(tmp.path());
        let resolved = resolve_icon(
            &mut saved,
            "k",
            Some(&project),
            Some("/cached/hint.png".into()),
        );

        assert_eq!(
            resolved,
            Some(project.join("logo.svg").to_string_lossy().into_owned())
        );
    }

    /// Nothing on disk and no hint: the caller gets None and draws initials.
    /// Nothing is remembered either — an entry pointing at nothing would be
    /// re-read next launch as a project with a broken icon.
    #[test]
    fn resolves_to_nothing_when_there_is_nothing() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let project = tmp.path().join("proj");
        std::fs::create_dir_all(&project).expect("project dir");

        let mut saved = scratch_saved(tmp.path());
        let resolved = resolve_icon(&mut saved, "k", Some(&project), None);

        assert_eq!(resolved, None);
        assert_eq!(saved.get_icon("k"), None);
    }

    /// 0-byte placeholders must lose to nothing at all.
    #[test]
    fn ignores_empty_icon_files() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        std::fs::create_dir_all(&assets).expect("assets dir");
        std::fs::write(assets.join("favicon.svg"), b"").expect("write empty");

        assert_eq!(detect_icon(tmp.path()), None);
    }
}
