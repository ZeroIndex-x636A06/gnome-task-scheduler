//! OpenRC adapter — writes `/etc/local.d/<name>.start` shell scripts.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use task_scheduler_core::{
    ensure_not_protected, validate_name, Backend, Capabilities, Lifecycle, SchedulerError, Scope, Task, TaskOrigin,
    TaskScheduler, Trigger,
};



const LOCAL_D: &str = "/etc/local.d";

pub struct OpenRcRootAdapter {
    dir: PathBuf,
}

impl OpenRcRootAdapter {
    pub fn new() -> Self { Self { dir: PathBuf::from(LOCAL_D) } }

    fn require_root() -> Result<(), SchedulerError> {
        let euid = unsafe { libc::geteuid() };
        if euid != 0 { return Err(SchedulerError::NotRoot); }
        Ok(())
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.start"))
    }
}

impl Default for OpenRcRootAdapter {
    fn default() -> Self { Self::new() }
}

impl TaskScheduler for OpenRcRootAdapter {
    fn backend(&self) -> Backend { Backend::OpenRc }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            on_calendar: false,
            on_boot: true,
            on_login: false,
            on_device: false,
            enable_toggle: true,
            system_scope: true,
        }
    }

    fn create_task(&self, task: &Task) -> Result<(), SchedulerError> {
        Self::require_root()?;
        validate_name(&task.name)?;
        ensure_not_protected(&task.name)?;
        if !matches!(task.trigger, Trigger::OnBootSec(_)) {
            return Err(SchedulerError::UnsupportedTrigger(task.trigger.kind().into()));
        }
        fs::create_dir_all(&self.dir)?;
        let p = self.path(&task.name);
        let cleanup = match task.lifecycle {
            Lifecycle::Persistent => String::new(),
            Lifecycle::DisableAfterRun => format!("chmod 644 {}\n", p.display()),
            Lifecycle::DeleteAfterRun => format!("rm -f {}\n", p.display()),
        };
        let script = format!(
            "#!/bin/sh\n# task-scheduler:{name}\n{cmd}\nstatus=$?\n{cleanup}exit $status\n",
            name = task.name,
            cmd = task.command,
            cleanup = cleanup,
        );
        fs::write(&p, script)?;
        let mode = if task.enabled { 0o755 } else { 0o644 };
        fs::set_permissions(&p, fs::Permissions::from_mode(mode))?;
        Ok(())
    }


    fn delete_task(&self, name: &str) -> Result<(), SchedulerError> {
        Self::require_root()?;
        validate_name(name)?;
        ensure_not_protected(name)?;
        let p = self.path(name);
        if p.exists() { fs::remove_file(&p)?; }
        Ok(())
    }

    fn toggle_task(&self, name: &str, enable: bool) -> Result<(), SchedulerError> {
        Self::require_root()?;
        validate_name(name)?;
        ensure_not_protected(name)?;
        let p = self.path(name);
        if !p.exists() {
            return Err(SchedulerError::CommandFailed(format!("missing: {}", p.display())));
        }
        let mode = if enable { 0o755 } else { 0o644 };
        fs::set_permissions(&p, fs::Permissions::from_mode(mode))?;
        Ok(())
    }

    fn list_tasks(&self) -> Result<Vec<Task>, SchedulerError> {
        if !self.dir.exists() { return Ok(vec![]); }
        let mut tasks = Vec::new();
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            let path = entry.path();
            let Some(name_os) = path.file_name() else { continue; };
            let fname = name_os.to_string_lossy().to_string();
            let Some(stem) = fname.strip_suffix(".start") else { continue; };
            let body = fs::read_to_string(&path).unwrap_or_default();
            let origin = if body.contains(&format!("# task-scheduler:{stem}")) {
                TaskOrigin::Owned
            } else {
                TaskOrigin::Foreign
            };
            let command = body
                .lines()
                .find(|l| !l.starts_with("#!") && !l.trim_start().starts_with('#') && !l.trim().is_empty())
                .unwrap_or("")
                .to_string();
            let enabled = entry
                .metadata()
                .ok()
                .map(|m| m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false);
            tasks.push(Task {
                name: stem.to_string(),
                command,
                trigger: Trigger::OnBootSec("boot".into()),
                enabled,
                next_run: None,
                scope: Scope::System,
                backend: Backend::OpenRc,
                origin,
                lifecycle: task_scheduler_core::Lifecycle::default(),
                has_snapshot_previous: false,
                has_snapshot_original: false,
            });
        }
        tasks.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(tasks)
    }
}

