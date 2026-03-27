use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::config::loader;
use crate::config::projects::SavedProjects;
use crate::config::schema::ProcessConfig;
use crate::detect::detector::{self, DetectedStack};
use crate::process::auto_restart;
use crate::process::manager::{ProcessManager, ProcessManagerRef};
use crate::util::icon_detector;
use crate::watcher::file_watcher::FileWatcher;

pub struct Project {
    pub name: String,
    pub dir: PathBuf,
    pub manager: ProcessManagerRef,
    pub icon_path: Option<String>,
    pub _file_watcher: Option<FileWatcher>,
}

pub struct PreparedProject {
    pub name: String,
    pub dir: PathBuf,
    pub dir_string: String,
    pub manager: ProcessManagerRef,
    pub stacks: Vec<DetectedStack>,
    pub config_loaded: bool,
}

pub type WorkspaceRef = Rc<RefCell<Workspace>>;

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

    /// Prepare a project for loading: detect stacks but don't add detected processes yet.
    /// Returns None if the project is already loaded.
    pub fn prepare_project(&mut self, dir: &Path) -> Option<PreparedProject> {
        let dir_str = dir.to_string_lossy().to_string();
        if self
            .projects
            .iter()
            .any(|p| p.dir.to_string_lossy() == dir_str)
        {
            log::info!("Project already loaded: {}", dir.display());
            return None;
        }

        let manager = ProcessManager::new();
        let mut project_name = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "project".to_string());

        let dir_string = dir.to_string_lossy().to_string();

        if let Some(custom_name) = self.saved.get_name(&dir_string) {
            project_name = custom_name.clone();
        }

        let mut config_loaded = false;
        let mut stacks = Vec::new();

        if let Some(config_path) = loader::find_config(dir) {
            match loader::load_config(&config_path) {
                Ok(config) => {
                    if self.saved.get_name(&dir_string).is_none() {
                        project_name = config.project.name.clone();
                    }
                    let mut mgr = manager.borrow_mut();
                    for mut proc_config in config.process {
                        if proc_config.working_dir.is_none() {
                            proc_config.working_dir = Some(dir_string.clone());
                        }
                        mgr.add_process(proc_config);
                    }
                    config_loaded = true;
                    log::info!("Loaded config from {}", config_path.display());
                }
                Err(e) => log::error!("Failed to load config: {e}"),
            }
        } else {
            log::info!(
                "No tuxflow.toml, running stack detection in {}",
                dir.display()
            );
            stacks = detector::detect_stacks(dir);
            for stack in &stacks {
                log::info!(
                    "Detected stack: {} ({} commands)",
                    stack.name,
                    stack.suggested_processes.len()
                );
            }
        }

        Some(PreparedProject {
            name: project_name,
            dir: dir.to_path_buf(),
            dir_string,
            manager,
            stacks,
            config_loaded,
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
            dir,
            dir_string,
            manager,
            ..
        } = prepared;

        // Add the selected detected processes
        {
            let mut mgr = manager.borrow_mut();
            for mut pc in selected_processes {
                if pc.working_dir.is_none() {
                    pc.working_dir = Some(dir_string.clone());
                }
                mgr.add_process(pc);
            }
        }

        // Load user-added custom commands
        if let Some(custom_cmds) = self.saved.get_custom_commands(&dir_string) {
            let mut mgr = manager.borrow_mut();
            for cmd in custom_cmds.clone() {
                mgr.add_process(cmd);
            }
        }

        // Filter out previously deleted processes
        {
            let mgr = manager.borrow();
            let to_remove: Vec<String> = mgr
                .process_names()
                .iter()
                .filter(|name| self.saved.is_process_deleted(&dir_string, name))
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

        let icon_path = self.saved.get_icon(&dir_string).cloned().or_else(|| {
            let detected = icon_detector::detect_icon(&dir);
            if let Some(ref path) = detected {
                log::info!("Auto-detected project icon: {path}");
                self.saved.set_icon(&dir_string, Some(path.clone()));
            }
            detected
        });

        // Start file watcher for restart_when_changed patterns
        let file_watcher = FileWatcher::new(&dir, &manager);

        let project = Project {
            name: project_name,
            dir,
            manager,
            icon_path,
            _file_watcher: file_watcher,
        };

        self.saved.add(&dir_string);

        self.projects.push(project);
        self.projects.last()
    }

    /// Convenience: prepare + finalize with detected processes (used for startup/CLI loading).
    /// Makefile targets are only included if the user previously curated them via the selection
    /// dialog (indicated by Make-related entries in deleted_processes). Otherwise they are
    /// excluded at startup to avoid spawning many VTE terminals for projects with large Makefiles.
    pub fn add_project_from_dir(&mut self, dir: &Path) -> Option<&Project> {
        let prepared = self.prepare_project(dir)?;
        let dir_string = dir.to_string_lossy().to_string();
        let has_make_curation = self
            .saved
            .has_deleted_processes_matching(&dir_string, "make ");
        let processes: Vec<ProcessConfig> = prepared
            .stacks
            .iter()
            .filter(|s| s.name != "Make" || prepared.config_loaded || has_make_curation)
            .flat_map(|s| s.suggested_processes.clone())
            .collect();
        self.finalize_project(prepared, processes)
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
            let dir_str = project.dir.to_string_lossy().to_string();
            project.manager.borrow_mut().stop_all();
            self.saved.remove(&dir_str);
            self.projects.remove(idx);
        }
    }

    pub fn rename_project(&mut self, old_name: &str, new_name: &str) {
        if let Some(project) = self.projects.iter_mut().find(|p| p.name == old_name) {
            let dir_str = project.dir.to_string_lossy().to_string();
            project.name = new_name.to_string();
            self.saved.set_name(&dir_str, new_name);
        }
    }

    pub fn get_project_dir(&self, project_name: &str) -> Option<PathBuf> {
        self.projects
            .iter()
            .find(|p| p.name == project_name)
            .map(|p| p.dir.clone())
    }

    pub fn set_project_icon(&mut self, project_name: &str, icon_path: Option<String>) {
        if let Some(project) = self.projects.iter_mut().find(|p| p.name == project_name) {
            let dir_str = project.dir.to_string_lossy().to_string();
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
            let dir_str = project.dir.to_string_lossy().to_string();
            self.saved.set_process_order(&dir_str, order);
        }
    }

    pub fn set_project_expanded(&mut self, project_name: &str, expanded: bool) {
        if let Some(project) = self.projects.iter().find(|p| p.name == project_name) {
            let dir_str = project.dir.to_string_lossy().to_string();
            self.saved.set_expanded(&dir_str, expanded);
        }
    }

    pub fn is_project_expanded(&self, project_name: &str) -> Option<bool> {
        self.projects
            .iter()
            .find(|p| p.name == project_name)
            .and_then(|p| self.saved.is_expanded(p.dir.to_string_lossy().as_ref()))
    }

    pub fn save_custom_command(
        &mut self,
        project_name: &str,
        config: crate::config::schema::ProcessConfig,
    ) {
        if let Some(project) = self.projects.iter().find(|p| p.name == project_name) {
            let dir_str = project.dir.to_string_lossy().to_string();
            self.saved.add_custom_command(&dir_str, config);
        }
    }

    pub fn set_display_name(&mut self, project_name: &str, process_name: &str, display_name: &str) {
        if let Some(project) = self.projects.iter().find(|p| p.name == project_name) {
            let dir_str = project.dir.to_string_lossy().to_string();
            self.saved
                .set_display_name(&dir_str, process_name, display_name);
        }
    }

    pub fn mark_process_deleted(&mut self, project_name: &str, process_name: &str) {
        if let Some(project) = self.projects.iter().find(|p| p.name == project_name) {
            let dir_str = project.dir.to_string_lossy().to_string();
            // Remove from custom commands if it was user-added
            self.saved.remove_custom_command(&dir_str, process_name);
            // Mark as deleted so auto-detected ones don't reappear
            self.saved.add_deleted_process(&dir_str, process_name);
        }
    }

    pub fn mark_process_deleted_by_dir(&mut self, dir: &str, process_name: &str) {
        self.saved.add_deleted_process(dir, process_name);
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
        self.saved.reorder_to_match(
            &self
                .projects
                .iter()
                .map(|p| p.dir.to_string_lossy().to_string())
                .collect::<Vec<_>>(),
        );
    }
}

/// Create a qualified name: "project::process"
pub fn qualified_name(project: &str, process: &str) -> String {
    format!("{project}::{process}")
}
