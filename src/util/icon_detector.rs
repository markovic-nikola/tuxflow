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
        let dir = std::env::temp_dir().join(format!("tuxflow-icon-test-{}", std::process::id()));
        let assets = dir.join("assets");
        std::fs::create_dir_all(&assets).expect("temp dir");
        std::fs::write(assets.join("favicon.svg"), b"<svg/>").expect("write icon");

        let found = detect_icon(&dir);
        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(
            found,
            Some(assets.join("favicon.svg").to_string_lossy().into_owned())
        );
    }

    /// 0-byte placeholders must lose to nothing at all.
    #[test]
    fn ignores_empty_icon_files() {
        let dir = std::env::temp_dir().join(format!("tuxflow-icon-empty-{}", std::process::id()));
        let assets = dir.join("assets");
        std::fs::create_dir_all(&assets).expect("temp dir");
        std::fs::write(assets.join("favicon.svg"), b"").expect("write empty");

        let found = detect_icon(&dir);
        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(found, None);
    }
}
