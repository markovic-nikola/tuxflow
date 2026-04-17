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

/// Try to find a project icon by checking common file locations.
/// Returns the absolute path to the first match found.
pub fn detect_icon(project_dir: &Path) -> Option<String> {
    for candidate in CANDIDATES {
        if candidate.contains('*') {
            if let Ok(matches) = glob::glob(&project_dir.join(candidate).to_string_lossy())
                && let Some(Ok(path)) = matches.into_iter().next()
            {
                return Some(path.to_string_lossy().to_string());
            }
        } else {
            let path = project_dir.join(candidate);
            if path.is_file() {
                return Some(path.to_string_lossy().to_string());
            }
        }
    }
    None
}
