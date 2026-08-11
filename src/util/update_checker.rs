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

/// How long a check is reused before asking GitHub again.
///
/// Short on purpose. Restarting the app is what people do when they want to
/// know about a new version, and a long window makes that silently do nothing
/// — a release stayed invisible for hours with no way to ask for a re-check.
/// The limit this guards against (60 anonymous requests/hour/IP) is nowhere
/// near reachable at four checks an hour, so the earlier six-hour window cost
/// real usefulness to save nothing.
const CHECK_INTERVAL: Duration = Duration::from_secs(15 * 60);

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
    // Same path cleaning as the relaunch: after an in-place upgrade the raw
    // /proc/self/exe carries a " (deleted)" marker, and asking dpkg about that
    // path fails — which used to hide the install button from then on.
    let Ok(exe) = relaunch_path() else {
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

/// What the kernel appends to `/proc/self/exe` once the inode we are running
/// has lost its last name on disk.
const DELETED_MARKER: &str = " (deleted)";

/// Strip the `" (deleted)"` marker the kernel appends to `/proc/self/exe`
/// once the package upgrade has replaced the binary under us. Rust hands the
/// link target back verbatim, so the path it returns does not exist and
/// spawning it fails — the whole point of restarting is that the file changed.
fn clean_exe_path(raw: &str) -> &str {
    raw.strip_suffix(DELETED_MARKER).unwrap_or(raw)
}

/// True once the binary this process was exec'd from has been replaced or
/// removed — an apt/dpkg upgrade landing under a running window, typically
/// from the system's software manager.
///
/// dpkg unpacks to `<path>.dpkg-new` and renames over the original, so our
/// inode loses its last name and the kernel starts marking the link deleted.
/// The old code stays mapped and the window keeps working, which is exactly
/// the problem: without this there is nothing to tell the user that the
/// version on disk has moved on. A `readlink` per call, so it is fine to poll.
pub fn binary_replaced() -> bool {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().ends_with(DELETED_MARKER))
        .unwrap_or(false)
}

/// Absolute path of the binary to relaunch.
fn relaunch_path() -> Result<PathBuf, String> {
    let raw = std::env::current_exe().map_err(|e| format!("{e}"))?;
    let cleaned = PathBuf::from(clean_exe_path(&raw.to_string_lossy()));
    if cleaned.is_file() {
        return Ok(cleaned);
    }
    // Upgraded to a different location, or an exotic /proc: fall back to PATH.
    let name = cleaned.file_name().unwrap_or_default().to_owned();
    std::env::var_os("PATH")
        .map(|p| {
            std::env::split_paths(&p)
                .map(|d| d.join(&name))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
        .into_iter()
        .find(|c| c.is_file())
        .ok_or_else(|| format!("Could not find {} to relaunch", cleaned.display()))
}

/// Relaunch after this process exits.
///
/// The app is single-instance (GtkApplication holds a D-Bus name), so a
/// process started while we are still alive just activates *us* and exits —
/// which looked exactly like the restart button doing nothing. Hand off to a
/// detached shell that waits for our PID to disappear and then execs the new
/// binary.
pub fn restart() -> Result<(), String> {
    use std::os::unix::process::CommandExt;

    let exe = relaunch_path()?;
    let pid = std::process::id();
    let script = format!(
        "while kill -0 {pid} 2>/dev/null; do sleep 0.2; done; exec {}",
        crate::remote::sh_quote(&exe.to_string_lossy())
    );

    std::process::Command::new("sh")
        .arg("-c")
        .arg(script)
        // Inherited, this makes stack detection treat every project as
        // TuxFlow itself (see detect::detector).
        .env_remove("TUXFLOW_CHILD")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        // Own process group, so tearing this one down cannot take the
        // relauncher with it.
        .process_group(0)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not restart: {e}"))
}

#[cfg(test)]
mod tests {

    #[test]
    fn strips_the_deleted_marker_from_a_replaced_binary() {
        // What /proc/self/exe reads back after apt swaps the file underneath.
        assert_eq!(
            clean_exe_path("/usr/bin/tuxflow (deleted)"),
            "/usr/bin/tuxflow"
        );
        assert_eq!(clean_exe_path("/usr/bin/tuxflow"), "/usr/bin/tuxflow");
        // Only a trailing marker counts; a real path may contain the word.
        assert_eq!(
            clean_exe_path("/home/me/my (deleted) app/tuxflow"),
            "/home/me/my (deleted) app/tuxflow"
        );
    }

    #[test]
    fn an_intact_binary_is_not_reported_as_replaced() {
        // The test runner's own exe is still linked, so nothing to flag.
        assert!(!binary_replaced());
    }

    #[test]
    fn relaunch_path_resolves_to_a_real_file() {
        let p = relaunch_path().expect("test binary must resolve");
        assert!(p.is_file(), "{} is not a file", p.display());
    }
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
