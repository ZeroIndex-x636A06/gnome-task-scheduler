//! `~/.config/autostart/<name>.desktop` adapter for login triggers.
//!
//! `list_tasks` enumerates both the user dir and `/etc/xdg/autostart/`
//! (system-wide foreign entries are read-only at the source). Files in the
//! user dir without our `Comment=task-scheduler` marker are tagged Foreign.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use task_scheduler_core::{
    is_owned_desktop_text, snapshot, ensure_not_protected, validate_name, Backend, Capabilities, Lifecycle,
    SchedulerError, Scope, SnapshotTarget, Task, TaskOrigin, TaskScheduler, Trigger,
};


pub struct AutostartAdapter {
    dir: PathBuf,
}

impl AutostartAdapter {
    pub fn new() -> Self {
        let dir = dirs::config_dir()
            .map(|d| d.join("autostart"))
            .unwrap_or_else(|| PathBuf::from("/tmp/autostart-fallback"));
        Self { dir }
    }

    fn user_path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.desktop"))
    }
    fn snapshot_root(&self) -> PathBuf { snapshot::snapshot_root(Scope::User) }

    fn xdg_dirs() -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        let cfg = std::env::var("XDG_CONFIG_DIRS").unwrap_or_else(|_| "/etc/xdg".into());
        for d in cfg.split(':').filter(|s| !s.is_empty()) {
            out.push(PathBuf::from(d).join("autostart"));
        }
        out
    }

    fn take_snapshot(&self, name: &str, slot: &str) -> Result<(), SchedulerError> {
        let p = self.user_path(name);
        snapshot::write_slot(
            &self.snapshot_root(),
            Backend::Autostart.tag(),
            name,
            slot,
            &[p.as_path()],
        )
    }
}

impl Default for AutostartAdapter { fn default() -> Self { Self::new() } }

impl TaskScheduler for AutostartAdapter {
    fn backend(&self) -> Backend { Backend::Autostart }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            on_calendar: false,
            on_boot: false,
            on_login: true,
            on_device: false,
            enable_toggle: true,
            system_scope: false,
        }
    }

    fn create_task(&self, task: &Task) -> Result<(), SchedulerError> {
        validate_name(&task.name)?;
        ensure_not_protected(&task.name)?;
        if !matches!(task.trigger, Trigger::OnLogin) {
            return Err(SchedulerError::UnsupportedTrigger(task.trigger.kind().into()));
        }
        // Snapshot existing file before overwriting.
        let snap_root = self.snapshot_root();
        let tag = Backend::Autostart.tag();
        let exists = self.user_path(&task.name).exists();
        if exists && !snapshot::has_slot(&snap_root, tag, &task.name, "original") {
            let _ = self.take_snapshot(&task.name, "original");
        }
        if exists {
            let _ = self.take_snapshot(&task.name, "previous");
        }

        // Lifecycle wrapper: rewrite the .desktop file (or remove it) after
        // the user's command runs.
        let exec_field = match write_autostart_wrapper(&task.name, &task.command, task.lifecycle)? {
            Some(p) => format!("/bin/sh {}", p.display()),
            None => task.command.clone(),
        };

        fs::create_dir_all(&self.dir)?;
        let body = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name={name}\n\
             Comment=task-scheduler\n\
             Exec={cmd}\n\
             X-GNOME-Autostart-enabled={enabled}\n\
             Hidden={hidden}\n",
            name = task.name,
            cmd = exec_field,
            enabled = if task.enabled { "true" } else { "false" },
            hidden = if task.enabled { "false" } else { "true" },
        );
        fs::write(self.user_path(&task.name), body)?;
        Ok(())
    }

    fn delete_task(&self, name: &str) -> Result<(), SchedulerError> {
        validate_name(name)?;
        ensure_not_protected(name)?;
        let p = self.user_path(name);
        if p.exists() {
            let _ = self.take_snapshot(name, "previous");
            fs::remove_file(&p)?;
        }
        let _ = remove_autostart_wrapper(name);
        Ok(())
    }


    fn toggle_task(&self, name: &str, enable: bool) -> Result<(), SchedulerError> {
        validate_name(name)?;
        ensure_not_protected(name)?;
        let p = self.user_path(name);
        // Foreign entries in /etc/xdg/autostart can be disabled by writing
        // a stub override in the user dir.
        let body = if p.exists() {
            fs::read_to_string(&p)?
        } else {
            // Look up foreign source so we can mirror it as user-level override.
            let mut found = None;
            for dir in Self::xdg_dirs() {
                let candidate = dir.join(format!("{name}.desktop"));
                if candidate.exists() {
                    found = Some(fs::read_to_string(&candidate)?);
                    break;
                }
            }
            let Some(text) = found else {
                return Err(SchedulerError::CommandFailed(format!("autostart not found: {name}")));
            };
            fs::create_dir_all(&self.dir)?;
            text
        };
        let mut out = String::with_capacity(body.len());
        let mut saw_enabled = false;
        let mut saw_hidden = false;
        for line in body.lines() {
            if let Some(_v) = line.strip_prefix("X-GNOME-Autostart-enabled=") {
                out.push_str(&format!("X-GNOME-Autostart-enabled={}\n", if enable { "true" } else { "false" }));
                saw_enabled = true;
            } else if let Some(_v) = line.strip_prefix("Hidden=") {
                out.push_str(&format!("Hidden={}\n", if enable { "false" } else { "true" }));
                saw_hidden = true;
            } else {
                out.push_str(line); out.push('\n');
            }
        }
        if !saw_enabled {
            out.push_str(&format!("X-GNOME-Autostart-enabled={}\n", if enable { "true" } else { "false" }));
        }
        if !saw_hidden {
            out.push_str(&format!("Hidden={}\n", if enable { "false" } else { "true" }));
        }
        fs::write(self.user_path(name), out)?;
        Ok(())
    }

    fn revert_task(&self, name: &str, target: SnapshotTarget) -> Result<(), SchedulerError> {
        validate_name(name)?;
        ensure_not_protected(name)?;
        let snap_root = self.snapshot_root();
        let tag = Backend::Autostart.tag();
        let _ = fs::remove_file(self.user_path(name));
        snapshot::restore_slot(&snap_root, tag, name, target.as_str(), &self.dir)?;
        if matches!(target, SnapshotTarget::Original) {
            let _ = snapshot::purge_task_snapshots(&snap_root, tag, name);
        }
        Ok(())
    }

    fn list_tasks(&self) -> Result<Vec<Task>, SchedulerError> {
        let mut tasks = Vec::new();
        let snap_root = self.snapshot_root();
        let tag = Backend::Autostart.tag();

        // 1) User dir (Owned or Foreign depending on marker).
        if self.dir.exists() {
            for entry in fs::read_dir(&self.dir)? {
                let entry = entry?;
                let path = entry.path();
                let Some(name_os) = path.file_name() else { continue; };
                let fname = name_os.to_string_lossy().to_string();
                let Some(stem) = fname.strip_suffix(".desktop") else { continue; };
                let body = fs::read_to_string(&path).unwrap_or_default();
                let command = field(&body, "Exec").unwrap_or_default();
                if command.is_empty() { continue; }
                let enabled = field(&body, "X-GNOME-Autostart-enabled")
                    .map(|v| v == "true")
                    .unwrap_or_else(|| field(&body, "Hidden").map(|v| v != "true").unwrap_or(true));
                let origin = if is_owned_desktop_text(&body) {
                    TaskOrigin::Owned
                } else {
                    TaskOrigin::Foreign
                };
                tasks.push(Task {
                    name: stem.to_string(),
                    command,
                    trigger: Trigger::OnLogin,
                    enabled,
                    next_run: None,
                    scope: Scope::User,
                    backend: Backend::Autostart,
                    origin,
                    lifecycle: task_scheduler_core::Lifecycle::default(),
                    has_snapshot_previous: snapshot::has_slot(&snap_root, tag, stem, "previous"),
                    has_snapshot_original: snapshot::has_slot(&snap_root, tag, stem, "original"),
                });
            }
        }

        // 2) System XDG dirs — always Foreign. Skip ones already overridden
        // in the user dir.
        let user_names: std::collections::HashSet<String> =
            tasks.iter().map(|t| t.name.clone()).collect();
        for dir in Self::xdg_dirs() {
            let Ok(rd) = fs::read_dir(&dir) else { continue };
            for entry in rd.flatten() {
                let path = entry.path();
                let Some(name_os) = path.file_name() else { continue; };
                let fname = name_os.to_string_lossy().to_string();
                let Some(stem) = fname.strip_suffix(".desktop") else { continue; };
                if user_names.contains(stem) { continue; }
                let body = fs::read_to_string(&path).unwrap_or_default();
                let command = field(&body, "Exec").unwrap_or_default();
                if command.is_empty() { continue; }
                let enabled = field(&body, "Hidden").map(|v| v != "true").unwrap_or(true);
                tasks.push(Task {
                    name: stem.to_string(),
                    command,
                    trigger: Trigger::OnLogin,
                    enabled,
                    next_run: None,
                    scope: Scope::User,
                    backend: Backend::Autostart,
                    origin: TaskOrigin::Foreign,
                    lifecycle: task_scheduler_core::Lifecycle::default(),
                    has_snapshot_previous: false,
                    has_snapshot_original: false,
                });
            }
        }

        tasks.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(tasks)
    }
}

fn field(body: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    let mut in_entry = false;
    for line in body.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_entry = t == "[Desktop Entry]";
            continue;
        }
        if !in_entry { continue; }
        if let Some(v) = t.strip_prefix(&prefix) {
            return Some(v.trim().to_string());
        }
    }
    None
}

fn autostart_wrapper_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("task-scheduler").join("wrappers"))
}

fn autostart_wrapper_path(name: &str) -> Option<PathBuf> {
    autostart_wrapper_dir().map(|d| d.join(format!("autostart-{name}.sh")))
}

fn write_autostart_wrapper(
    name: &str,
    command: &str,
    lifecycle: Lifecycle,
) -> Result<Option<PathBuf>, SchedulerError> {
    if lifecycle == Lifecycle::Persistent {
        return Ok(None);
    }
    let dir = autostart_wrapper_dir().ok_or(SchedulerError::NoConfigDir)?;
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("autostart-{name}.sh"));
    let desktop_path_expr = format!("\"$HOME/.config/autostart/{name}.desktop\"");
    let cleanup = match lifecycle {
        Lifecycle::DisableAfterRun => format!(
            "sed -i 's/^Hidden=false/Hidden=true/; s/^X-GNOME-Autostart-enabled=true/X-GNOME-Autostart-enabled=false/' {desktop_path_expr}"
        ),
        Lifecycle::DeleteAfterRun => format!(
            "rm -f {desktop_path_expr} '{wrapper}'",
            wrapper = path.display()
        ),
        Lifecycle::Persistent => unreachable!(),
    };
    let body = format!(
        "#!/bin/sh\n# task-scheduler autostart wrapper for {name}\n{command}\nstatus=$?\n{cleanup}\nexit $status\n"
    );
    fs::write(&path, body)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
    Ok(Some(path))
}

fn remove_autostart_wrapper(name: &str) -> std::io::Result<()> {
    if let Some(p) = autostart_wrapper_path(name) {
        if p.exists() {
            return fs::remove_file(p);
        }
    }
    Ok(())
}
