//! Pulling a remote project's icon down to where a shell can render it.
//!
//! Neither GUI can draw a file that lives on another machine, so the icon is
//! detected on the host in one batched round trip and copied into
//! `~/.cache/tuxflow/icons/`; everything above this module then renders a
//! remote project's icon from that local copy.
//!
//! What projects.toml SAVES, though, is where the file lives on the host —
//! `ssh://host/abs/path` (a [`host_ref`]) — never the cache copy. That file
//! is synced between machines and the cache is not: a saved cache path is a
//! dead entry on every machine but the one that fetched it, and a hand-
//! picked icon outside the detection candidates can't be re-detected there
//! either, so it fell back to initials. The cache path is derived from the
//! ref ([`cache_path`]) and refetched wherever it is missing.
//!
//! Older releases saved the cache path itself. Such a legacy entry is
//! rewritten as a ref by whichever machine still has the file
//! ([`IconPlan::Identify`]); a machine without it shows a detected icon but
//! leaves the entry alone, so it can't clobber a pick the other machine is
//! about to identify.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::remote::fs::SshFs;
use crate::remote::{ProjectLocation, sh_quote, ssh_mux_options};
use crate::util::icon_detector;

/// Icons are small; 2 MB guards against something mislabeled as one.
const MAX_ICON_BYTES: usize = 2 * 1024 * 1024;

/// A remote icon on this machine: the local copy to render, and the
/// [`host_ref`] projects.toml saves.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteIcon {
    pub local: String,
    pub source: String,
}

/// The saved form of a file on `host`: `ssh://host/abs/path` — the same
/// shape as a remote project key, so [`ProjectLocation::parse`] reads it.
pub fn host_ref(host: &str, abs_path: &str) -> String {
    ProjectLocation::Ssh {
        host: host.to_string(),
        dir: abs_path.to_string(),
    }
    .key()
}

/// The host file a saved icon names, when it is a [`host_ref`].
fn parse_host_ref(saved: &str) -> Option<(String, String)> {
    match ProjectLocation::parse(saved) {
        ProjectLocation::Ssh { host, dir } => Some((host, dir)),
        ProjectLocation::Local(_) => None,
    }
}

/// Where the local copy of `abs_path` for `project_key` lives: named by the
/// project key so a re-fetch overwrites the same slot, and by the source's
/// extension so the renderer can tell an .ico from an .svg.
pub fn cache_path(project_key: &str, abs_path: &str) -> Option<PathBuf> {
    let ext = abs_path.rsplit('.').next().unwrap_or("png");
    Some(cache_dir()?.join(format!("{:016x}.{ext}", crate::remote::fnv64(project_key))))
}

/// The local file a saved icon value renders from: a [`host_ref`]'s cache
/// copy, or the value itself (a local project's icon, a legacy entry).
pub fn local_path(saved: &str, project_key: &str) -> Option<PathBuf> {
    match parse_host_ref(saved) {
        Some((_, abs)) => cache_path(project_key, &abs),
        None => Some(PathBuf::from(saved)),
    }
}

/// What a remote project's probe does about its icon, decided on the UI
/// thread from the saved entry and run on the probe's worker by [`run`].
#[derive(Debug, Clone, PartialEq)]
pub enum IconPlan {
    /// The saved icon's local copy is on disk — no round trip.
    Ready,
    /// A [`host_ref`] with no copy on this machine (another machine picked
    /// it, or the cache was cleared): fetch exactly that file.
    Fetch { abs: String },
    /// Detect on the host. `persist` is false for a legacy entry whose file
    /// is gone: the detection is shown but not saved, since the machine that
    /// still has the file will identify the real pick and sync it.
    Detect { persist: bool },
    /// A legacy cache-path entry whose file IS here: find which host file it
    /// is a copy of, so the entry can become a ref every machine can fetch.
    Identify { local: PathBuf },
}

/// Decide the probe's icon work for a remote project.
pub fn plan(saved: Option<&str>, project_key: &str) -> IconPlan {
    let Some(saved) = saved else {
        return IconPlan::Detect { persist: true };
    };
    let usable = |p: &Path| icon_detector::is_usable_icon(p);
    match parse_host_ref(saved) {
        Some((_, abs)) => match cache_path(project_key, &abs) {
            Some(local) if usable(&local) => IconPlan::Ready,
            _ => IconPlan::Fetch { abs },
        },
        None if usable(Path::new(saved)) => IconPlan::Identify {
            local: PathBuf::from(saved),
        },
        None => IconPlan::Detect { persist: false },
    }
}

/// What [`run`] found: the icon, and whether its ref should be saved.
#[derive(Debug, Clone, PartialEq)]
pub struct Fetched {
    pub icon: RemoteIcon,
    pub persist: bool,
}

/// Carry out a [`plan`] for the project at `host:dir`. Best-effort; None
/// when there is nothing to show beyond what the saved entry already gives.
/// Blocking — call from a worker thread.
pub fn run(plan: &IconPlan, host: &str, dir: &str) -> Option<Fetched> {
    let key = host_ref(host, dir);
    match plan {
        IconPlan::Ready => None,
        IconPlan::Fetch { abs } => match cache_remote_icon(host, abs, &key) {
            Some(icon) => Some(Fetched {
                icon,
                persist: false,
            }),
            // The picked file is gone from the host too: show a detection,
            // but keep the entry — it may come back, and it is the user's.
            None => fetch_remote_icon(host, dir).map(|icon| Fetched {
                icon,
                persist: false,
            }),
        },
        IconPlan::Detect { persist } => fetch_remote_icon(host, dir).map(|icon| Fetched {
            icon,
            persist: *persist,
        }),
        IconPlan::Identify { local } => {
            let abs = identify_source(host, dir, local)?;
            Some(Fetched {
                icon: RemoteIcon {
                    local: local.to_string_lossy().into_owned(),
                    source: host_ref(host, &abs),
                },
                persist: true,
            })
        }
    }
}

/// Detect a project icon on the host and copy it into the local cache.
/// Candidates are tried in priority order until one actually downloads — a
/// single unreadable file must not cost the project its icon. Best-effort.
/// Blocking — call from a worker thread.
pub fn fetch_remote_icon(host: &str, dir: &str) -> Option<RemoteIcon> {
    let key = host_ref(host, dir);
    let fs = SshFs::new(host, dir);
    for rel in icon_detector::detect_icons_fs(&fs) {
        let abs = format!("{}/{}", dir.trim_end_matches('/'), rel);
        match cache_remote_icon(host, &abs, &key) {
            Some(icon) => return Some(icon),
            None => log::info!("Remote icon candidate {host}:{abs} didn't fetch; trying next"),
        }
    }
    None
}

/// Download `abs_path` from `host` into its [`cache_path`]. Returns the
/// local copy a shell can render alongside the ref to save. Blocking — call
/// from a worker thread.
pub fn cache_remote_icon(host: &str, abs_path: &str, project_key: &str) -> Option<RemoteIcon> {
    let bytes = crate::remote::fs::fetch_remote_file(host, abs_path, MAX_ICON_BYTES)?;
    let path = cache_path(project_key, abs_path)?;
    std::fs::create_dir_all(path.parent()?).ok()?;
    std::fs::write(&path, &bytes).ok()?;
    log::info!(
        "Fetched remote project icon {host}:{abs_path} -> {}",
        path.display()
    );
    Some(RemoteIcon {
        local: path.to_string_lossy().into_owned(),
        source: host_ref(host, abs_path),
    })
}

/// Find the file under `dir` on `host` that `local` is a copy of — the
/// legacy-entry migration. A hand pick can be anywhere in the tree, so this
/// searches rather than trying the detection candidates, but cheaply: one
/// `find` for files of exactly `local`'s size and extension (dependency and
/// VCS trees pruned), then byte comparison of the few that come back.
fn identify_source(host: &str, dir: &str, local: &Path) -> Option<String> {
    let bytes = std::fs::read(local).ok()?;
    let ext = local.extension()?.to_str()?;
    let cmd = format!(
        "find {} -maxdepth 6 \\( -name node_modules -o -name vendor -o -name .git \\) -prune \
         -o -type f -size {}c -name {} -print 2>/dev/null | LC_ALL=C sort | head -n 20",
        sh_quote(dir),
        bytes.len(),
        sh_quote(&format!("*.{ext}")),
    );
    let out = Command::new("ssh")
        .args(ssh_mux_options())
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"])
        .arg(host)
        .arg("--")
        .arg(cmd)
        .output()
        .ok()?;
    let listed = String::from_utf8_lossy(&out.stdout);
    let found = prefer_usual_places(dir, listed.lines().filter(|l| l.starts_with('/')))
        .into_iter()
        .find(|abs| {
            crate::remote::fs::fetch_remote_file(host, abs, MAX_ICON_BYTES)
                .is_some_and(|b| b == bytes)
        })
        .map(str::to_string);
    match &found {
        Some(abs) => log::info!("Cached icon {} is {host}:{abs}", local.display()),
        None => log::info!("No file on {host} matches cached icon {}", local.display()),
    }
    found
}

/// Order identical-looking matches so the one that lasts wins: detection's
/// candidates first (in its priority order), then shallower paths — a
/// `build/` or `dist/` copy has the same bytes as `public/logo.png` and
/// sorts ahead of it, but the next build deletes it.
fn prefer_usual_places<'a>(dir: &str, paths: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let root = format!("{}/", dir.trim_end_matches('/'));
    let mut paths: Vec<&str> = paths.collect();
    paths.sort_by_key(|abs| {
        let rel = abs.strip_prefix(&root).unwrap_or(abs);
        (
            icon_detector::candidate_rank(rel).unwrap_or(usize::MAX),
            rel.matches('/').count(),
            *abs,
        )
    });
    paths
}

/// Where cached remote icons live.
fn cache_dir() -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("tuxflow/icons"))
}

/// Delete a saved icon's local copy, but only if it is one we downloaded —
/// a removed project must not orphan cache files, while an icon detected
/// *inside* a project tree is the project's own and not ours to delete.
pub fn discard_if_cached(saved: &str, project_key: &str) {
    if let (Some(path), Some(dir)) = (local_path(saved, project_key), cache_dir())
        && path.starts_with(dir)
    {
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "ssh://box/srv/app";

    #[test]
    fn a_host_ref_round_trips() {
        let saved = host_ref("box", "/srv/app/public/logo.png");
        assert_eq!(saved, "ssh://box/srv/app/public/logo.png");
        assert_eq!(
            parse_host_ref(&saved),
            Some(("box".into(), "/srv/app/public/logo.png".into()))
        );
        assert_eq!(parse_host_ref("/home/me/.cache/tuxflow/icons/x.png"), None);
    }

    /// The local copy of a ref is derived, so every machine agrees on it
    /// without the cache path ever reaching the synced file.
    #[test]
    fn a_ref_renders_from_its_derived_cache_copy() {
        let saved = host_ref("box", "/srv/app/public/logo.png");
        let local = local_path(&saved, KEY).expect("cache dir");
        assert_eq!(local, cache_path(KEY, "/srv/app/public/logo.png").unwrap());
        assert!(local.to_string_lossy().ends_with(".png"));
        // A plain path (local project, legacy entry) renders as itself.
        assert_eq!(local_path("/a/b.svg", KEY), Some(PathBuf::from("/a/b.svg")));
    }

    #[test]
    fn identification_prefers_candidates_then_shallow_paths() {
        let found = [
            "/srv/app/build/client/logo.png",
            "/srv/app/resources/img/x/logo.png",
            "/srv/app/img/logo.png",
            "/srv/app/public/logo.png",
        ];
        assert_eq!(
            prefer_usual_places("/srv/app", found.into_iter()),
            [
                "/srv/app/public/logo.png",
                "/srv/app/img/logo.png",
                "/srv/app/build/client/logo.png",
                "/srv/app/resources/img/x/logo.png",
            ]
        );
    }

    #[test]
    fn no_entry_detects_and_saves() {
        assert_eq!(plan(None, KEY), IconPlan::Detect { persist: true });
    }

    /// The laptop case: the other machine picked the icon, so the entry is a
    /// ref and this cache has no copy — fetch exactly the pick.
    #[test]
    fn a_ref_without_a_copy_fetches_that_file() {
        let abs = "/srv/app/no-such-copy-on-this-machine.png";
        assert_eq!(
            plan(Some(&host_ref("box", abs)), "ssh://box/srv/never-fetched"),
            IconPlan::Fetch { abs: abs.into() }
        );
    }

    #[test]
    fn a_legacy_entry_with_its_file_is_identified() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let file = tmp.path().join("abc.png");
        std::fs::write(&file, b"png").expect("write icon");
        let saved = file.to_string_lossy().into_owned();
        assert_eq!(plan(Some(&saved), KEY), IconPlan::Identify { local: file });
    }

    /// A legacy entry whose file is gone shows a detection but must not
    /// overwrite the synced entry — the machine with the file identifies it.
    #[test]
    fn a_dead_legacy_entry_detects_without_saving() {
        assert_eq!(
            plan(Some("/gone/cleared-cache.png"), KEY),
            IconPlan::Detect { persist: false }
        );
    }
}
