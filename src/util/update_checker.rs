use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct UpdateInfo {
    pub latest_version: String,
    pub release_url: String,
    /// GitHub's generated release notes, shown before installing.
    #[serde(default)]
    pub notes: String,
    /// Direct .deb URL, when the release carries one. Absent for source-only
    /// releases, which fall back to opening the release page.
    #[serde(default)]
    pub deb_url: Option<String>,
}

/// Unauthenticated GitHub allows 60 requests/hour per IP — shared across
/// everyone behind a NAT. Checking on every launch burned that for no reason,
/// so the last answer is cached and re-used.
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(serde::Serialize, serde::Deserialize)]
struct Cached {
    checked_at: u64,
    /// Latest version seen, whether or not it was newer than ours at the time
    /// (the running binary can be older after a downgrade).
    info: Option<UpdateInfo>,
}

fn cache_path() -> Option<PathBuf> {
    let dir = dirs::cache_dir()?.join("tuxflow");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("update-check.json"))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn read_cache() -> Option<Cached> {
    let raw = std::fs::read_to_string(cache_path()?).ok()?;
    serde_json::from_str(&raw).ok()
}

fn write_cache(info: Option<&UpdateInfo>) {
    let Some(path) = cache_path() else { return };
    let cached = Cached {
        checked_at: now_secs(),
        info: info.cloned(),
    };
    if let Ok(json) = serde_json::to_string(&cached) {
        let _ = std::fs::write(path, json);
    }
}

/// Latest release, from cache when it is fresh enough. Blocking — call from a
/// worker thread. Returns `None` when we are already up to date.
pub fn check_for_update() -> Option<UpdateInfo> {
    let current = env!("CARGO_PKG_VERSION");

    if let Some(cached) = read_cache()
        && now_secs().saturating_sub(cached.checked_at) < CHECK_INTERVAL.as_secs()
    {
        return cached.info.filter(|i| is_newer(&i.latest_version, current));
    }

    let info = fetch_latest(current);
    write_cache(info.as_ref());
    info.filter(|i| is_newer(&i.latest_version, current))
}

fn fetch_latest(current: &str) -> Option<UpdateInfo> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(5)))
        .build();
    let agent: ureq::Agent = config.into();

    let mut response = agent
        .get("https://api.github.com/repos/markovic-nikola/tuxflow/releases/latest")
        .header("Accept", "application/vnd.github.v3+json")
        .header("User-Agent", &format!("tuxflow/{current}"))
        .call()
        .ok()?;

    let body: serde_json::Value = response.body_mut().read_json().ok()?;
    let tag = body.get("tag_name")?.as_str()?;
    let latest = tag.strip_prefix('v').unwrap_or(tag);
    let url = body
        .get("html_url")
        .and_then(|v: &serde_json::Value| v.as_str())
        .unwrap_or("https://github.com/markovic-nikola/tuxflow/releases")
        .to_string();
    let notes = body
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let deb_url = body
        .get("assets")
        .and_then(|a| a.as_array())
        .and_then(|assets| {
            assets.iter().find_map(|a| {
                let name = a.get("name")?.as_str()?;
                name.ends_with("_amd64.deb")
                    .then(|| a.get("browser_download_url")?.as_str().map(String::from))
                    .flatten()
            })
        });

    Some(UpdateInfo {
        latest_version: latest.to_string(),
        release_url: url,
        notes,
        deb_url,
    })
}

/// Numeric-only comparison. A component that isn't a plain number (a `-rc1`
/// suffix, say) makes the whole comparison bail rather than silently dropping
/// the component — `0.2.0-rc1` parsed as `[0, 2]` used to outrank `0.1.54`.
fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |v: &str| -> Option<Vec<u32>> { v.split('.').map(|s| s.parse().ok()).collect() };
    match (parse(latest), parse(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

/// Where the running binary came from, which decides how we can update it.
pub enum InstallKind {
    /// Owned by dpkg — replaceable in place with apt.
    Deb,
    /// Tarball, `cargo run`, or anything else: hand off to the browser.
    Other,
}

pub fn install_kind() -> InstallKind {
    let Ok(exe) = std::env::current_exe() else {
        return InstallKind::Other;
    };
    let owned = std::process::Command::new("dpkg")
        .arg("-S")
        .arg(&exe)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if owned {
        InstallKind::Deb
    } else {
        InstallKind::Other
    }
}

/// Download the .deb to a temp path. Blocking.
pub fn download_deb(url: &str) -> Result<PathBuf, String> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(120)))
        .build();
    let agent: ureq::Agent = config.into();

    let mut response = agent
        .get(url)
        .header(
            "User-Agent",
            &format!("tuxflow/{}", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .map_err(|e| format!("Download failed: {e}"))?;

    let mut bytes = Vec::new();
    std::io::copy(&mut response.body_mut().as_reader(), &mut bytes)
        .map_err(|e| format!("Download failed: {e}"))?;

    let path = std::env::temp_dir().join(format!("tuxflow-update-{}.deb", now_secs()));
    std::fs::write(&path, &bytes)
        .map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    Ok(path)
}

/// Install a .deb with one polkit prompt. `apt-get install` (not `dpkg -i`)
/// so dependencies resolve — a plain dpkg install fails on a machine missing
/// libvte/libadwaita. Blocking.
pub fn install_deb(path: &Path) -> Result<(), String> {
    // Absolute path: pkexec runs with a sanitised environment, and this branch
    // is only reached on a dpkg-owned install, so apt-get is where it always is.
    let output = std::process::Command::new("pkexec")
        .arg("/usr/bin/apt-get")
        .arg("install")
        .arg("-y")
        .arg("--allow-downgrades")
        .arg(path)
        .output()
        .map_err(|e| format!("Could not run pkexec: {e}"))?;

    if output.status.success() {
        return Ok(());
    }
    // 126/127 are polkit's "dismissed" and "not authorised".
    match output.status.code() {
        Some(126) | Some(127) => Err("Authentication cancelled".to_string()),
        _ => {
            let err = String::from_utf8_lossy(&output.stderr);
            Err(err.lines().last().unwrap_or("Install failed").to_string())
        }
    }
}

/// Re-exec the (now replaced) binary and let the caller quit.
pub fn restart() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("{e}"))?;
    std::process::Command::new(exe)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not restart: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_newer() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
        // Numeric ordering, not lexicographic.
        assert!(is_newer("0.1.100", "0.1.99"));
    }

    #[test]
    fn pre_release_tags_do_not_outrank_stable() {
        // Used to parse as [0, 2] and beat 0.1.54.
        assert!(!is_newer("0.2.0-rc1", "0.1.54"));
        assert!(!is_newer("v0.2.0", "0.1.54"));
        assert!(!is_newer("nightly", "0.1.54"));
    }
}
