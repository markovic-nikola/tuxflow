//! Restart-on-file-change: which process a changed file belongs to, and
//! the debounced directory watcher that reports changes. Shared by both
//! shells so the glob rules exist once — GTK feeds the reports through a
//! GLib timeout, iced through a subscription stream, but "does `src/a.rs`
//! restart `dev`?" must answer the same in each.
//!
//! Patterns are `restart_when_changed` globs matched against the path
//! RELATIVE to the project root (`src/**/*.rs`, `config/*.toml`) with the
//! glob crate's defaults, where `*` crosses `/` (so `*.md` is any markdown
//! file at any depth); a path outside the root is matched as-is. A process fires at most once per
//! changed path however many of its patterns match. Local projects only:
//! a remote project's files live on the host, where nothing of ours runs
//! to watch them.

use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::RecursiveMode;
use notify_debouncer_mini::{DebounceEventResult, DebouncedEventKind, Debouncer, new_debouncer};

use crate::config::schema::ProcessConfig;

/// Events closer together than this collapse into one report — an editor
/// save writes a temp file, renames it and touches metadata in quick
/// succession, and each would otherwise restart the process.
pub const DEBOUNCE: Duration = Duration::from_millis(500);

/// The form field's text (comma-separated globs) → the config list.
/// Blanks are dropped, so `a, ,b,` is two patterns.
pub fn parse_patterns(text: &str) -> Vec<String> {
    text.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

struct Entry {
    process: String,
    patterns: Vec<glob::Pattern>,
}

/// Every process of a project that watches something, with its compiled
/// patterns. Built from the live entry list, so it is cheap to rebuild
/// whenever that list changes.
#[derive(Default)]
pub struct WatchSet {
    entries: Vec<Entry>,
}

impl WatchSet {
    /// Processes with at least one VALID pattern; an unparsable glob is
    /// logged and skipped rather than disabling the whole process's list.
    pub fn from_configs<'a>(configs: impl IntoIterator<Item = &'a ProcessConfig>) -> Self {
        let entries = configs
            .into_iter()
            .filter(|c| !c.restart_when_changed.is_empty())
            .filter_map(|c| {
                let patterns: Vec<glob::Pattern> = c
                    .restart_when_changed
                    .iter()
                    .filter_map(|g| match glob::Pattern::new(g) {
                        Ok(p) => Some(p),
                        Err(e) => {
                            log::warn!("{}: bad watch pattern {g:?}: {e}", c.name);
                            None
                        }
                    })
                    .collect();
                (!patterns.is_empty()).then(|| Entry {
                    process: c.name.clone(),
                    patterns,
                })
            })
            .collect();
        Self { entries }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The patterns as saved, in entry order — a stable identity for "did
    /// the watch configuration change", which is what decides whether a
    /// running watcher must be rebuilt.
    pub fn signature(&self) -> Vec<(String, Vec<String>)> {
        self.entries
            .iter()
            .map(|e| {
                (
                    e.process.clone(),
                    e.patterns.iter().map(|p| p.as_str().to_string()).collect(),
                )
            })
            .collect()
    }

    /// Names of the processes `path` restarts, each at most once, in
    /// entry order.
    pub fn matches(&self, project_dir: &Path, path: &Path) -> Vec<&str> {
        let relative = path.strip_prefix(project_dir).unwrap_or(path);
        let rel = relative.to_string_lossy();
        self.entries
            .iter()
            .filter(|e| e.patterns.iter().any(|p| p.matches(&rel)))
            .map(|e| e.process.as_str())
            .collect()
    }

    /// `matches` over a debounced batch: the union, deduped, in entry
    /// order — a save touching ten files under one pattern is ONE restart.
    pub fn matches_any<'a>(
        &self,
        project_dir: &Path,
        paths: impl IntoIterator<Item = &'a PathBuf>,
    ) -> Vec<&str> {
        let mut hit = vec![false; self.entries.len()];
        for path in paths {
            let relative = path.strip_prefix(project_dir).unwrap_or(path);
            let rel = relative.to_string_lossy();
            for (i, e) in self.entries.iter().enumerate() {
                if !hit[i] && e.patterns.iter().any(|p| p.matches(&rel)) {
                    hit[i] = true;
                }
            }
        }
        self.entries
            .iter()
            .zip(hit)
            .filter(|(_, h)| *h)
            .map(|(e, _)| e.process.as_str())
            .collect()
    }
}

/// A running recursive watch on a project directory. Dropping it stops
/// the watcher thread.
pub struct Watcher {
    _debouncer: Debouncer<notify::RecommendedWatcher>,
}

/// Watch `project_dir` recursively and hand every debounced batch of
/// changed paths to `on_change`, which runs on the watcher's own thread —
/// route it to the UI thread there, don't touch UI state. `None` when the
/// watch could not be established (a vanished directory, inotify limits).
pub fn start(
    project_dir: &Path,
    mut on_change: impl FnMut(Vec<PathBuf>) + Send + 'static,
) -> Option<Watcher> {
    let mut debouncer = new_debouncer(DEBOUNCE, move |result: DebounceEventResult| match result {
        Ok(events) => {
            let paths: Vec<PathBuf> = events
                .into_iter()
                .filter(|e| e.kind == DebouncedEventKind::Any)
                .map(|e| e.path)
                .collect();
            if !paths.is_empty() {
                on_change(paths);
            }
        }
        Err(e) => log::warn!("file watcher: {e}"),
    })
    .map_err(|e| log::warn!("file watcher: {e}"))
    .ok()?;
    debouncer
        .watcher()
        .watch(project_dir, RecursiveMode::Recursive)
        .map_err(|e| log::warn!("file watcher on {}: {e}", project_dir.display()))
        .ok()?;
    log::info!("file watcher started in {}", project_dir.display());
    Some(Watcher {
        _debouncer: debouncer,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::ProcessCategory;

    fn cfg(name: &str, patterns: &[&str]) -> ProcessConfig {
        ProcessConfig {
            name: name.into(),
            command: name.into(),
            working_dir: None,
            start_with_project: false,
            auto_restart: false,
            open_in_browser: false,
            restart_when_changed: patterns.iter().map(|s| s.to_string()).collect(),
            env: Default::default(),
            category: ProcessCategory::Command,
            auto_named: false,
            display_name: None,
        }
    }

    #[test]
    fn parse_drops_blanks_and_trims() {
        assert_eq!(
            parse_patterns(" a.rs , ,src/**/*.rs,"),
            ["a.rs", "src/**/*.rs"]
        );
        assert!(parse_patterns("  ").is_empty());
    }

    #[test]
    fn matches_relative_to_the_project_root() {
        let set = WatchSet::from_configs([&cfg("dev", &["src/**/*.rs"]), &cfg("docs", &["*.md"])]);
        let root = Path::new("/p");
        assert_eq!(set.matches(root, Path::new("/p/src/a/b.rs")), ["dev"]);
        assert_eq!(set.matches(root, Path::new("/p/README.md")), ["docs"]);
        // glob's default: `*` crosses `/` — `*.md` means "any markdown
        // file anywhere", which is how GTK's watcher always read it.
        assert_eq!(set.matches(root, Path::new("/p/src/x.md")), ["docs"]);
        assert!(set.matches(root, Path::new("/p/src/x.txt")).is_empty());
        // Outside the root: matched as-is, so nothing here fires.
        assert!(
            set.matches(root, Path::new("/elsewhere/src/a.rs"))
                .is_empty()
        );
    }

    #[test]
    fn a_process_fires_once_however_many_patterns_hit() {
        let set = WatchSet::from_configs([&cfg("dev", &["src/*", "src/*.rs", "**/*.rs"])]);
        assert_eq!(
            set.matches(Path::new("/p"), Path::new("/p/src/a.rs")),
            ["dev"]
        );
    }

    #[test]
    fn a_batch_dedupes_across_paths_in_entry_order() {
        let set = WatchSet::from_configs([&cfg("dev", &["src/**"]), &cfg("web", &["*.css"])]);
        let root = Path::new("/p");
        let paths = [
            PathBuf::from("/p/a.css"),
            PathBuf::from("/p/src/a.rs"),
            PathBuf::from("/p/src/b.rs"),
        ];
        assert_eq!(set.matches_any(root, &paths), ["dev", "web"]);
    }

    #[test]
    fn empty_and_bad_patterns_do_not_watch() {
        assert!(WatchSet::from_configs([&cfg("dev", &[])]).is_empty());
        assert!(WatchSet::from_configs([&cfg("dev", &["[bad"])]).is_empty());
        // One bad pattern doesn't sink the good one beside it.
        let set = WatchSet::from_configs([&cfg("dev", &["[bad", "*.rs"])]);
        assert_eq!(
            set.signature(),
            [("dev".to_string(), vec!["*.rs".to_string()])]
        );
    }
}
