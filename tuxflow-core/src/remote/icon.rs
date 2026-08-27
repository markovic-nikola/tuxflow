//! Pulling a remote project's icon down to where a shell can render it.
//!
//! Neither GUI can draw a file that lives on another machine, so the icon is
//! detected on the host in one batched round trip and copied into
//! `~/.cache/tuxflow/icons/`; everything above this module then treats a
//! remote project's icon as an ordinary local path.

use crate::remote::fs::SshFs;
use crate::util::icon_detector;

/// Detect a project icon on the host and copy it into the local cache.
/// Candidates are tried in priority order until one actually downloads — a
/// single unreadable file must not cost the project its icon. Best-effort.
/// Blocking — call from a worker thread.
pub fn fetch_remote_icon(host: &str, dir: &str) -> Option<String> {
    let key = format!("ssh://{host}{dir}");
    let fs = SshFs::new(host, dir);
    for rel in icon_detector::detect_icons_fs(&fs) {
        let abs = format!("{}/{}", dir.trim_end_matches('/'), rel);
        match cache_remote_icon(host, &abs, &key) {
            Some(path) => return Some(path),
            None => log::info!("Remote icon candidate {host}:{abs} didn't fetch; trying next"),
        }
    }
    None
}

/// Download `abs_path` from `host` into `~/.cache/tuxflow/icons/`, named by
/// the project key so re-fetches overwrite the same slot. Returns the local
/// path a shell can render. Blocking — call from a worker thread.
pub fn cache_remote_icon(host: &str, abs_path: &str, project_key: &str) -> Option<String> {
    // Icons are small; 2 MB guards against something mislabeled as one.
    let bytes = crate::remote::fs::fetch_remote_file(host, abs_path, 2 * 1024 * 1024)?;
    let ext = abs_path.rsplit('.').next().unwrap_or("png");
    let cache_dir = cache_dir()?;
    std::fs::create_dir_all(&cache_dir).ok()?;
    let path = cache_dir.join(format!("{:016x}.{ext}", crate::remote::fnv64(project_key)));
    std::fs::write(&path, &bytes).ok()?;
    log::info!(
        "Fetched remote project icon {host}:{abs_path} -> {}",
        path.display()
    );
    Some(path.to_string_lossy().into_owned())
}

/// Where cached remote icons live.
fn cache_dir() -> Option<std::path::PathBuf> {
    Some(dirs::cache_dir()?.join("tuxflow/icons"))
}

/// Delete an icon file, but only if it is one we downloaded — a removed
/// project must not orphan cache files, while an icon detected *inside* a
/// project tree is the project's own and not ours to delete.
pub fn discard_if_cached(icon_path: &str) {
    let path = std::path::PathBuf::from(icon_path);
    if let Some(dir) = cache_dir()
        && path.starts_with(dir)
    {
        let _ = std::fs::remove_file(&path);
    }
}
