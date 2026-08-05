use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::config::loader;
use crate::config::projects::SavedProjects;
use crate::config::schema::{ProcessCategory, ProcessConfig, TuxFlowConfig};
use crate::detect::detector::{self, DetectedStack};
use crate::process::manager::{ProcessManager, ProcessManagerRef};
use crate::remote::ProjectLocation;
use crate::remote::fs::{ProjectFs, SshFs};
use crate::util::icon_detector;
use crate::watcher::file_watcher::FileWatcher;

pub struct Project {
    pub name: String,
    pub location: ProjectLocation,
    pub manager: ProcessManagerRef,
    pub icon_path: Option<String>,
    /// Whether a tuxflow.toml was found and parsed at load time.
    pub config_loaded: bool,
    /// Stacks detected when the project was loaded. Kept so UI that needs
    /// suggestions later (Edit Project) has something for remote projects,
    /// where a live re-detection would mean ssh round trips on the UI thread.
    pub detected_stacks: Vec<DetectedStack>,
    pub _file_watcher: Option<FileWatcher>,
}

impl Project {
    /// Opaque key used for all per-project persisted state.
    pub fn key(&self) -> String {
        self.location.key()
    }

    /// Local directory, only for actions that operate on the local
    /// filesystem (git UI, open-in-editor, reveal). None when remote.
    pub fn local_dir(&self) -> Option<PathBuf> {
        match &self.location {
            ProjectLocation::Local(p) => Some(p.clone()),
            ProjectLocation::Ssh { .. } => None,
        }
    }
}

pub struct PreparedProject {
    pub name: String,
    pub location: ProjectLocation,
    pub key: String,
    pub manager: ProcessManagerRef,
    pub stacks: Vec<DetectedStack>,
    pub config_loaded: bool,
    /// Icon already fetched to a local path during a remote probe;
    /// None for local projects (they detect directly from disk).
    pub icon_hint: Option<String>,
}

/// Everything fetched from a remote host during project preparation.
/// Produced on a worker thread (ssh round trips), consumed on the main thread.
pub struct RemoteProbe {
    pub config: Option<TuxFlowConfig>,
    pub stacks: Vec<DetectedStack>,
    /// tmux sessions alive on the host — processes still running detached
    /// from a previous app run; the loader reattaches them.
    pub live_sessions: Vec<String>,
    /// Local path of a project icon fetched from the host into the cache
    /// (GTK can only render local files). Only probed when the project has
    /// no saved icon yet.
    pub icon_path: Option<String>,
}

/// Why a remote probe failed — decides whether retrying makes sense.
#[derive(Debug, Clone)]
pub enum ProbeError {
    /// ssh couldn't reach or authenticate to the host. Transient: the
    /// startup loader retries these in the background.
    Unreachable(String),
    /// The host answered but the project itself is bad (missing directory,
    /// broken config). Retrying won't help.
    Invalid(String),
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(msg) | Self::Invalid(msg) => f.write_str(msg),
        }
    }
}

/// Probe a remote project dir over ssh: read tuxflow.toml or run stack
/// detection, and (when `fetch_icon`) pull a project icon into the local
/// cache. Blocking — call from a worker thread, never the GTK main thread.
pub fn probe_remote(
    host: &str,
    dir: &str,
    conservative: bool,
    fetch_icon: bool,
) -> Result<RemoteProbe, ProbeError> {
    match crate::remote::fs::remote_dir_exists(host, dir) {
        Ok(true) => {}
        Ok(false) => {
            return Err(ProbeError::Invalid(format!(
                "No such directory on {host}: {dir}"
            )));
        }
        Err(e) => return Err(ProbeError::Unreachable(e)),
    }
    let live_sessions = crate::remote::list_live_sessions(host);
    let fs = SshFs::new(host, dir);
    let icon_path = if fetch_icon {
        fetch_remote_icon(host, dir, &fs)
    } else {
        None
    };
    if let Ok(content) = fs.read_to_string("tuxflow.toml") {
        return match loader::load_config_str(&content) {
            Ok(config) => Ok(RemoteProbe {
                config: Some(config),
                stacks: Vec::new(),
                live_sessions,
                icon_path,
            }),
            Err(e) => {
                return Err(ProbeError::Invalid(format!(
                    "Failed to parse tuxflow.toml on {host}: {e}"
                )));
            }
        };
    }
    let stacks = if conservative {
        detector::detect_stacks_conservative_fs(&fs)
    } else {
        detector::detect_stacks_fs(&fs)
    };
    Ok(RemoteProbe {
        config: None,
        stacks,
        live_sessions,
        icon_path,
    })
}

/// Detect a project icon on the host (one batched round trip) and copy it
/// into `~/.cache/tuxflow/icons/` so GTK can render it. Best-effort.
fn fetch_remote_icon(host: &str, dir: &str, fs: &SshFs) -> Option<String> {
    let rel = crate::util::icon_detector::detect_icon_fs(fs)?;
    let abs = format!("{}/{}", dir.trim_end_matches('/'), rel);
    // Icons are small; 2 MB guards against something mislabeled as one.
    let bytes = crate::remote::fs::fetch_remote_file(host, &abs, 2 * 1024 * 1024)?;
    let ext = rel.rsplit('.').next().unwrap_or("png");
    let cache_dir = dirs::cache_dir()?.join("tuxflow/icons");
    std::fs::create_dir_all(&cache_dir).ok()?;
    let key = format!("ssh://{host}{dir}");
    let path = cache_dir.join(format!("{:016x}.{ext}", crate::remote::fnv64(&key)));
    std::fs::write(&path, &bytes).ok()?;
    log::info!(
        "Fetched remote project icon {host}:{abs} -> {}",
        path.display()
    );
    Some(path.to_string_lossy().into_owned())
}

pub type WorkspaceRef = Rc<RefCell<Workspace>>;

/// Describes one command candidate shown in the Edit Project dialog's Commands section.
pub struct CommandToggleEntry {
    pub config: ProcessConfig,
    pub initial_on: bool,
    pub is_custom: bool,
    pub source_label: &'static str,
}

pub struct Workspace {
    projects: Vec<Project>,
    saved: SavedProjects,
}

impl Workspace {
    pub fn new() -> WorkspaceRef {
        Rc::new(RefCell::new(Self {
            projects: Vec::new(),
            saved: SavedProjects::load(),
        }))
    }

    pub fn saved_directories(&self) -> Vec<String> {
        self.saved.directories.clone()
    }

    /// Whether a project (by key) already has a persisted icon — remote
    /// probes skip the icon fetch when it does.
    pub fn has_saved_icon(&self, key: &str) -> bool {
        self.saved.get_icon(key).is_some()
    }

    /// Prepare a project for loading: detect stacks but don't add detected processes yet.
    /// Returns None if the project is already loaded. Uses the full detector (every script
    /// in package.json); see `prepare_project_conservative` for the startup variant.
    pub fn prepare_project(&mut self, dir: &Path) -> Option<PreparedProject> {
        self.prepare_project_inner(dir, false)
    }

    /// Like `prepare_project` but uses the conservative detector — the narrower pre-expansion
    /// set for legacy projects at startup, so previously-loaded projects don't gain scripts
    /// the user never saw.
    pub fn prepare_project_conservative(&mut self, dir: &Path) -> Option<PreparedProject> {
        self.prepare_project_inner(dir, true)
    }

    fn prepare_project_inner(&mut self, dir: &Path, conservative: bool) -> Option<PreparedProject> {
        let mut config = None;
        let mut stacks = Vec::new();

        if let Some(config_path) = loader::find_config(dir) {
            match loader::load_config(&config_path) {
                Ok(c) => {
                    config = Some(c);
                    log::info!("Loaded config from {}", config_path.display());
                }
                Err(e) => log::error!("Failed to load config: {e}"),
            }
        } else {
            log::info!(
                "No tuxflow.toml, running stack detection in {}",
                dir.display()
            );
            stacks = if conservative {
                detector::detect_stacks_conservative(dir)
            } else {
                detector::detect_stacks(dir)
            };
            for stack in &stacks {
                log::info!(
                    "Detected stack: {} ({} commands)",
                    stack.name,
                    stack.suggested_processes.len()
                );
            }
        }

        self.assemble_prepared(ProjectLocation::Local(dir.to_path_buf()), config, stacks)
    }

    /// Prepare a remote project from data already fetched off-thread.
    pub fn prepare_project_probed(
        &mut self,
        location: ProjectLocation,
        probe: RemoteProbe,
    ) -> Option<PreparedProject> {
        log::info!(
            "Preparing remote project {} (config: {}, detected stacks: {})",
            location.key(),
            probe.config.is_some(),
            probe.stacks.len()
        );
        let mut prepared = self.assemble_prepared(location, probe.config, probe.stacks)?;
        prepared.icon_hint = probe.icon_path;
        Some(prepared)
    }

    /// Shared assembly for local and remote preparation: dedupe check,
    /// manager creation, name resolution, config-process registration.
    fn assemble_prepared(
        &mut self,
        location: ProjectLocation,
        config: Option<TuxFlowConfig>,
        stacks: Vec<DetectedStack>,
    ) -> Option<PreparedProject> {
        let key = location.key();
        if self.projects.iter().any(|p| p.key() == key) {
            log::info!("Project already loaded: {key}");
            return None;
        }

        let manager = ProcessManager::new(location.clone());
        let mut project_name = location.base_name();
        if let Some(custom_name) = self.saved.get_name(&key) {
            project_name = custom_name.clone();
        }

        let mut config_loaded = false;
        if let Some(config) = config {
            if self.saved.get_name(&key).is_none() {
                project_name = config.project.name.clone();
            }
            let dir_str = location.dir_str();
            let mut mgr = manager.borrow_mut();
            for mut proc_config in config.process {
                if proc_config.working_dir.is_none() {
                    proc_config.working_dir = Some(dir_str.clone());
                }
                mgr.add_process(proc_config);
            }
            config_loaded = true;
        }

        Some(PreparedProject {
            name: project_name,
            location,
            key,
            manager,
            stacks,
            config_loaded,
            icon_hint: None,
        })
    }

    /// Finalize a prepared project by adding selected processes and completing setup.
    pub fn finalize_project(
        &mut self,
        prepared: PreparedProject,
        selected_processes: Vec<ProcessConfig>,
    ) -> Option<&Project> {
        let PreparedProject {
            name: project_name,
            location,
            key: dir_string,
            manager,
            config_loaded,
            stacks: detected_stacks,
            icon_hint,
        } = prepared;

        // Add the selected detected processes. working_dir is a path on the
        // machine that owns the files — the remote dir for ssh projects.
        {
            let default_dir = location.dir_str();
            let mut mgr = manager.borrow_mut();
            for mut pc in selected_processes {
                if pc.working_dir.is_none() {
                    pc.working_dir = Some(default_dir.clone());
                }
                mgr.add_process(pc);
            }
        }

        // Load user-added custom commands
        let custom_names: std::collections::HashSet<String> = self
            .saved
            .get_custom_commands(&dir_string)
            .map(|cmds| cmds.iter().map(|c| c.name.clone()).collect())
            .unwrap_or_default();
        if let Some(custom_cmds) = self.saved.get_custom_commands(&dir_string) {
            let mut mgr = manager.borrow_mut();
            for cmd in custom_cmds.clone() {
                mgr.add_process(cmd);
            }
        }

        // Filter out previously deleted auto-detected processes.
        // Only auto-detected processes (not in custom_commands) are filtered,
        // and matching is by name only — names are unique within a project.
        {
            let mgr = manager.borrow();
            let to_remove: Vec<String> = mgr
                .process_names()
                .iter()
                .filter(|name| {
                    !custom_names.contains(*name)
                        && self.saved.is_process_deleted(&dir_string, name)
                })
                .cloned()
                .collect();
            drop(mgr);
            let mut mgr = manager.borrow_mut();
            for name in &to_remove {
                mgr.remove_process(name);
            }
        }

        // Apply saved process order if available
        if let Some(saved_order) = self.saved.get_process_order(&dir_string) {
            manager.borrow_mut().apply_saved_order(saved_order);
        }

        // Auto-restart is set up lazily via on_materialized when terminals are created

        // Icon detection and file watching read the local filesystem —
        // both are skipped for remote projects (generic icon, no watch).
        let local_dir = match &location {
            ProjectLocation::Local(p) => Some(p.clone()),
            ProjectLocation::Ssh { .. } => None,
        };

        let icon_path = self.saved.get_icon(&dir_string).cloned().or_else(|| {
            // Local projects detect from disk; remote ones arrive with the
            // icon already fetched to the cache by the probe (icon_hint).
            let detected = local_dir
                .as_deref()
                .and_then(icon_detector::detect_icon)
                .or(icon_hint);
            if let Some(ref path) = detected {
                log::info!("Auto-detected project icon: {path}");
                self.saved.set_icon(&dir_string, Some(path.clone()));
            }
            detected
        });

        // Start file watcher for restart_when_changed patterns
        let file_watcher = local_dir
            .as_deref()
            .and_then(|dir| FileWatcher::new(dir, &manager));

        let project = Project {
            name: project_name,
            location,
            manager,
            icon_path,
            config_loaded,
            detected_stacks,
            _file_watcher: file_watcher,
        };

        self.saved.add(&dir_string);

        // Insert respecting the saved sidebar order: remote projects finish
        // their async probe at unpredictable times, so a plain push would
        // order projects by load completion instead of by the user's order.
        let saved_order = self.saved.directories.clone();
        let pos_of = |key: &str| {
            saved_order
                .iter()
                .position(|k| k == key)
                .unwrap_or(usize::MAX)
        };
        let my_pos = pos_of(&dir_string);
        let insert_idx = self
            .projects
            .iter()
            .position(|p| pos_of(&p.key()) > my_pos)
            .unwrap_or(self.projects.len());
        self.projects.insert(insert_idx, project);
        self.projects.get(insert_idx)
    }

    /// Convenience: prepare + finalize with detected processes (used for startup/CLI loading).
    ///
    /// Once a project has been curated (has a saved process_order), newly-detected commands
    /// that the user has never seen — not in process_order, custom_commands, or
    /// deleted_processes — are auto-marked as deleted so they don't appear unsolicited when
    /// the project's tooling changes (e.g. new Makefile targets, new npm scripts).
    pub fn add_project_from_dir(&mut self, dir: &Path) -> Option<&Project> {
        let prepared = self.prepare_project_conservative(dir)?;
        let processes = self.auto_select_processes(&prepared);
        self.finalize_project(prepared, processes)
    }

    /// The non-interactive selection used at startup: all detected processes,
    /// with never-seen commands auto-hidden once the project has been curated.
    pub fn auto_select_processes(&mut self, prepared: &PreparedProject) -> Vec<ProcessConfig> {
        let key = &prepared.key;
        let is_curated = self.saved.get_process_order(key).is_some();

        if is_curated && !prepared.config_loaded {
            let known: std::collections::HashSet<String> = self
                .saved
                .get_process_order(key)
                .map(|order| order.iter().cloned().collect())
                .unwrap_or_default();
            let custom: std::collections::HashSet<String> = self
                .saved
                .get_custom_commands(key)
                .map(|cmds| cmds.iter().map(|c| c.name.clone()).collect())
                .unwrap_or_default();
            for stack in &prepared.stacks {
                for proc in &stack.suggested_processes {
                    if !known.contains(&proc.name)
                        && !custom.contains(&proc.name)
                        && !self.saved.is_process_deleted(key, &proc.name)
                    {
                        self.saved.add_deleted_process(key, &proc.name);
                    }
                }
            }
        }

        prepared
            .stacks
            .iter()
            .flat_map(|s| s.suggested_processes.clone())
            .collect()
    }

    pub fn projects(&self) -> &[Project] {
        &self.projects
    }

    pub fn find_process_project<'a>(&self, qualified_name: &'a str) -> Option<(&'a str, &'a str)> {
        // Split "project::process" into parts
        if let Some((proj, proc_name)) = qualified_name.split_once("::") {
            Some((proj, proc_name))
        } else {
            None
        }
    }

    pub fn get_manager_for_project(&self, project_name: &str) -> Option<&ProcessManagerRef> {
        self.projects
            .iter()
            .find(|p| p.name == project_name)
            .map(|p| &p.manager)
    }

    pub fn remove_project(&mut self, project_name: &str) {
        if let Some(idx) = self.projects.iter().position(|p| p.name == project_name) {
            let project = &self.projects[idx];
            let dir_str = project.key();
            // A fetched remote icon lives in our cache — delete it with the
            // project so removals don't orphan cache files. Icons pointing
            // into the project tree (local detection) are left alone.
            if let (Some(icon), Some(cache_dir)) =
                (self.saved.get_icon(&dir_str), dirs::cache_dir())
            {
                let icon_path = PathBuf::from(icon);
                if icon_path.starts_with(cache_dir.join("tuxflow/icons")) {
                    let _ = std::fs::remove_file(&icon_path);
                }
            }
            project.manager.borrow_mut().stop_all();
            self.saved.remove(&dir_str);
            self.projects.remove(idx);
        }
    }

    pub fn set_project_name(&mut self, dir: &str, name: &str) {
        self.saved.set_name(dir, name);
    }

    pub fn rename_project(&mut self, old_name: &str, new_name: &str) {
        if let Some(project) = self.projects.iter_mut().find(|p| p.name == old_name) {
            let dir_str = project.key();
            project.name = new_name.to_string();
            self.saved.set_name(&dir_str, new_name);
        }
    }

    /// Local project directory — None for remote projects, which naturally
    /// gates local-only actions (git UI, open-in-editor, reveal) off for them.
    pub fn get_project_dir(&self, project_name: &str) -> Option<PathBuf> {
        self.projects
            .iter()
            .find(|p| p.name == project_name)
            .and_then(|p| p.local_dir())
    }

    pub fn get_project_location(&self, project_name: &str) -> Option<ProjectLocation> {
        self.projects
            .iter()
            .find(|p| p.name == project_name)
            .map(|p| p.location.clone())
    }

    pub fn set_project_icon(&mut self, project_name: &str, icon_path: Option<String>) {
        if let Some(project) = self.projects.iter_mut().find(|p| p.name == project_name) {
            let dir_str = project.key();
            project.icon_path = icon_path.clone();
            self.saved.set_icon(&dir_str, icon_path);
        }
    }

    pub fn get_project_icon(&self, project_name: &str) -> Option<String> {
        self.projects
            .iter()
            .find(|p| p.name == project_name)
            .and_then(|p| p.icon_path.clone())
    }

    pub fn save_process_order(&mut self, project_name: &str, order: Vec<String>) {
        if let Some(project) = self.projects.iter().find(|p| p.name == project_name) {
            let dir_str = project.key();
            self.saved.set_process_order(&dir_str, order);
        }
    }

    pub fn set_project_expanded(&mut self, project_name: &str, expanded: bool) {
        if let Some(project) = self.projects.iter().find(|p| p.name == project_name) {
            let dir_str = project.key();
            self.saved.set_expanded(&dir_str, expanded);
        }
    }

    pub fn is_project_expanded(&self, project_name: &str) -> Option<bool> {
        self.projects
            .iter()
            .find(|p| p.name == project_name)
            .and_then(|p| self.saved.is_expanded(&p.key()))
    }

    /// Stamp "now" as the project's last activity (drives the sidebar's
    /// recently-used sort). Persists immediately.
    pub fn touch_project_last_used(&mut self, project_name: &str) {
        if let Some(project) = self.projects.iter().find(|p| p.name == project_name) {
            let dir_str = project.key();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            self.saved.set_last_used(&dir_str, now);
        }
    }

    /// Unix seconds of the project's last activity; 0 = never used.
    pub fn project_last_used(&self, project_name: &str) -> u64 {
        self.projects
            .iter()
            .find(|p| p.name == project_name)
            .map(|p| self.saved.get_last_used(&p.key()))
            .unwrap_or(0)
    }

    pub fn save_custom_command(
        &mut self,
        project_name: &str,
        config: crate::config::schema::ProcessConfig,
    ) {
        if let Some(project) = self.projects.iter().find(|p| p.name == project_name) {
            let dir_str = project.key();
            self.saved.add_custom_command(&dir_str, config);
        }
    }

    pub fn set_display_name(&mut self, project_name: &str, process_name: &str, display_name: &str) {
        if let Some(project) = self.projects.iter().find(|p| p.name == project_name) {
            let dir_str = project.key();
            self.saved
                .set_display_name(&dir_str, process_name, display_name);
        }
    }

    pub fn mark_process_deleted(&mut self, project_name: &str, process_name: &str) {
        if let Some(project) = self.projects.iter().find(|p| p.name == project_name) {
            let dir_str = project.key();
            // Remove from custom commands if it was user-added
            self.saved.remove_custom_command(&dir_str, process_name);
            // Mark as deleted so auto-detected ones don't reappear
            self.saved.add_deleted_process(&dir_str, process_name);
        }
    }

    pub fn mark_process_deleted_by_dir(&mut self, dir: &str, process_name: &str) {
        self.saved.add_deleted_process(dir, process_name);
    }

    pub fn unmark_process_deleted(&mut self, project_name: &str, process_name: &str) {
        if let Some(project) = self.projects.iter().find(|p| p.name == project_name) {
            let dir_str = project.key();
            self.saved.unmark_process_deleted(&dir_str, process_name);
        }
    }

    /// Builds the union of commands for the Edit Project dialog:
    /// active (ON) → hidden (OFF, badge "hidden") → newly-detected (OFF, badge "new").
    /// Terminal and SSH categories are excluded. Deduped by name with active > hidden > new priority.
    pub fn list_toggleable_commands(&self, project_name: &str) -> Vec<CommandToggleEntry> {
        let Some(project) = self.projects.iter().find(|p| p.name == project_name) else {
            return Vec::new();
        };
        let dir_str = project.key();
        let mut entries: Vec<CommandToggleEntry> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

        // 1. Active processes (in manager, excluding Terminal/SSH)
        {
            let mgr = project.manager.borrow();
            for name in mgr.process_names() {
                if let Some(proc) = mgr.get_process(name)
                    && !matches!(
                        proc.config.category,
                        ProcessCategory::Terminal | ProcessCategory::SSH
                    )
                    && seen.insert(proc.config.name.clone())
                {
                    let is_custom = self
                        .saved
                        .get_custom_commands(&dir_str)
                        .is_some_and(|list| list.iter().any(|c| c.name == proc.config.name));
                    entries.push(CommandToggleEntry {
                        config: proc.config.clone(),
                        initial_on: true,
                        is_custom,
                        source_label: "active",
                    });
                }
            }
        }

        // Live re-detection would block the UI thread on ssh round trips for
        // remote projects — fall back to the stacks detected at load time
        // there, so hidden detected commands still resolve and can be
        // re-enabled. (Slightly stale: commands added to the remote project
        // since load appear after the next app restart.)
        let detected_now: Vec<DetectedStack> = match project.local_dir() {
            Some(dir) => detector::detect_stacks(&dir),
            None => project.detected_stacks.clone(),
        };

        // 2. Hidden commands (in deleted_processes), resolved from custom_commands or detection
        if let Some(deleted) = self.saved.deleted_processes.get(&dir_str) {
            for name in deleted {
                if seen.contains(name) {
                    continue;
                }
                let custom = self
                    .saved
                    .get_custom_commands(&dir_str)
                    .and_then(|list| list.iter().find(|c| &c.name == name).cloned());
                let (config, is_custom) = if let Some(c) = custom {
                    (c, true)
                } else if let Some(detected_cfg) = detected_now
                    .iter()
                    .flat_map(|s| &s.suggested_processes)
                    .find(|c| &c.name == name)
                {
                    (detected_cfg.clone(), false)
                } else {
                    continue;
                };
                if matches!(
                    config.category,
                    ProcessCategory::Terminal | ProcessCategory::SSH
                ) {
                    continue;
                }
                seen.insert(name.clone());
                entries.push(CommandToggleEntry {
                    config,
                    initial_on: false,
                    is_custom,
                    source_label: "hidden",
                });
            }
        }

        // 3. Newly detected commands not already in active or hidden
        for stack in &detected_now {
            for cfg in stack.suggested_processes.clone() {
                if seen.contains(&cfg.name) {
                    continue;
                }
                if matches!(
                    cfg.category,
                    ProcessCategory::Terminal | ProcessCategory::SSH
                ) {
                    continue;
                }
                seen.insert(cfg.name.clone());
                entries.push(CommandToggleEntry {
                    config: cfg,
                    initial_on: false,
                    is_custom: false,
                    source_label: "new",
                });
            }
        }

        entries
    }

    pub fn reorder_project(&mut self, project_name: &str, target_name: &str, before: bool) {
        let Some(src_idx) = self.projects.iter().position(|p| p.name == project_name) else {
            return;
        };
        let project = self.projects.remove(src_idx);
        let target_idx = self
            .projects
            .iter()
            .position(|p| p.name == target_name)
            .unwrap_or(0);
        let insert_idx = if before { target_idx } else { target_idx + 1 };
        self.projects.insert(insert_idx, project);
        self.saved
            .reorder_to_match(&self.projects.iter().map(|p| p.key()).collect::<Vec<_>>());
    }
}

/// Create a qualified name: "project::process"
pub fn qualified_name(project: &str, process: &str) -> String {
    format!("{project}::{process}")
}
