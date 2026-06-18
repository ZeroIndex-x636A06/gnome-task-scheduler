//! Root cron adapter — edits `/etc/crontab` in place.
//!
//! Each task we own is wrapped in a marker block so we can find/remove it
//! later without touching unrelated entries.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use task_scheduler_core::{
    systemd_calendar_to_cron, ensure_not_protected, validate_name, Backend, Capabilities, Lifecycle, SchedulerError,
    Scope, Task, TaskScheduler, Trigger,
};

const CRONTAB: &str = "/etc/crontab";
const MARKER_BEGIN: &str = "# >>> task-scheduler:";
const MARKER_END: &str = "# <<< task-scheduler:";
const WRAPPER_DIR: &str = "/var/lib/task-scheduler/wrappers";


pub struct CronRootAdapter {
    path: PathBuf,
}

impl CronRootAdapter {
    pub fn new() -> Self { Self { path: PathBuf::from(CRONTAB) } }

    fn require_root() -> Result<(), SchedulerError> {
        let euid = unsafe { libc::geteuid() };
        if euid != 0 { return Err(SchedulerError::NotRoot); }
        Ok(())
    }

    fn read(&self) -> Result<String, SchedulerError> {
        if !self.path.exists() { return Ok(String::new()); }
        Ok(fs::read_to_string(&self.path)?)
    }

    fn write(&self, contents: &str) -> Result<(), SchedulerError> {
        fs::write(&self.path, contents)?;
        Ok(())
    }
}

impl Default for CronRootAdapter {
    fn default() -> Self { Self::new() }
}

impl TaskScheduler for CronRootAdapter {
    fn backend(&self) -> Backend { Backend::CronSystem }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            on_calendar: true,
            on_boot: false,
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
        let Trigger::OnCalendar(expr) = &task.trigger else {
            return Err(SchedulerError::UnsupportedTrigger(task.trigger.kind().into()));
        };
        let cron = systemd_calendar_to_cron(expr).ok_or_else(|| {
            SchedulerError::Parse(format!(
                "cannot translate {expr} to cron; prefix with 'cron:'"
            ))
        })?;

        let cmd_field = match write_root_wrapper(&task.name, &task.command, task.lifecycle)? {
            Some(p) => format!("/bin/sh {}", p.display()),
            None => task.command.clone(),
        };

        let mut text = self.read()?;
        text = strip_block(&text, &task.name);
        // /etc/crontab uses 6 fields (with USER). Default to root.
        text.push_str(&format!(
            "{begin}{name}\n{cron} root {cmd}\n{end}{name}\n",
            begin = MARKER_BEGIN,
            end = MARKER_END,
            name = task.name,
            cron = cron,
            cmd = cmd_field,
        ));
        self.write(&text)
    }

    fn delete_task(&self, name: &str) -> Result<(), SchedulerError> {
        Self::require_root()?;
        validate_name(name)?;
        ensure_not_protected(name)?;
        let text = strip_block(&self.read()?, name);
        let _ = remove_root_wrapper(name);
        self.write(&text)
    }


    fn toggle_task(&self, name: &str, enable: bool) -> Result<(), SchedulerError> {
        Self::require_root()?;
        validate_name(name)?;
        ensure_not_protected(name)?;
        let text = self.read()?;
        let new = toggle_block(&text, name, enable);
        self.write(&new)
    }

    fn list_tasks(&self) -> Result<Vec<Task>, SchedulerError> {
        let text = self.read()?;
        Ok(parse_all(&text, Scope::System, Backend::CronSystem))
    }
}

// ---------- block helpers (shared shape used by cron adapters) ----------

pub(crate) fn strip_block(text: &str, name: &str) -> String {
    let begin = format!("{MARKER_BEGIN}{name}");
    let end = format!("{MARKER_END}{name}");
    let mut out = String::with_capacity(text.len());
    let mut inside = false;
    for line in text.lines() {
        if line.starts_with(&begin) { inside = true; continue; }
        if inside && line.starts_with(&end) { inside = false; continue; }
        if !inside {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

pub(crate) fn toggle_block(text: &str, name: &str, enable: bool) -> String {
    let begin = format!("{MARKER_BEGIN}{name}");
    let end = format!("{MARKER_END}{name}");
    let mut out = String::with_capacity(text.len());
    let mut inside = false;
    for line in text.lines() {
        if line.starts_with(&begin) { inside = true; out.push_str(line); out.push('\n'); continue; }
        if inside && line.starts_with(&end) { inside = false; out.push_str(line); out.push('\n'); continue; }
        if inside && !line.trim().is_empty() && !line.trim_start().starts_with('#') {
            if enable {
                out.push_str(line); out.push('\n');
            } else {
                out.push_str("#DISABLED# "); out.push_str(line); out.push('\n');
            }
        } else if inside && line.trim_start().starts_with("#DISABLED# ") {
            if enable {
                let rest = line.trim_start().trim_start_matches("#DISABLED# ");
                out.push_str(rest); out.push('\n');
            } else {
                out.push_str(line); out.push('\n');
            }
        } else {
            out.push_str(line); out.push('\n');
        }
    }
    out
}

/// Parse marker-wrapped owned blocks AND foreign cron lines.
pub(crate) fn parse_all(text: &str, scope: Scope, backend: Backend) -> Vec<Task> {
    use task_scheduler_core::TaskOrigin;
    let mut tasks = Vec::new();
    let mut foreign_counter = 0usize;
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some(name) = line.strip_prefix(MARKER_BEGIN) {
            let name = name.trim().to_string();
            let mut enabled = true;
            let mut expr = String::new();
            let mut cmd = String::new();
            i += 1;
            while i < lines.len() {
                let inner = lines[i];
                if inner.starts_with(MARKER_END) { i += 1; break; }
                let trimmed = inner.trim();
                if trimmed.is_empty() { i += 1; continue; }
                let body = if let Some(rest) = trimmed.strip_prefix("#DISABLED# ") {
                    enabled = false; rest
                } else if trimmed.starts_with('#') { i += 1; continue; } else { trimmed };
                let parts: Vec<&str> = body.splitn(7, char::is_whitespace).collect();
                if parts.len() >= 7 && scope == Scope::System {
                    expr = parts[..5].join(" ");
                    cmd = parts[6].to_string();
                } else if parts.len() >= 6 {
                    expr = parts[..5].join(" ");
                    cmd = parts[5].to_string();
                }
                i += 1;
            }
            tasks.push(Task {
                name, command: cmd,
                trigger: Trigger::OnCalendar(format!("cron:{expr}")),
                enabled, next_run: None, scope, backend,
                origin: TaskOrigin::Owned,
                lifecycle: task_scheduler_core::Lifecycle::default(),
                has_snapshot_previous: false,
                has_snapshot_original: false,
            });
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            i += 1; continue;
        }
        let parts: Vec<&str> = trimmed.splitn(7, char::is_whitespace).collect();
        let min_fields = if scope == Scope::System { 7 } else { 6 };
        if parts.len() >= min_fields && is_cron_field(parts[0]) {
            let expr = parts[..5].join(" ");
            let cmd = if scope == Scope::System {
                parts[6].to_string()
            } else {
                parts[5].to_string()
            };
            foreign_counter += 1;
            tasks.push(Task {
                name: format!("cron-{foreign_counter}"),
                command: cmd,
                trigger: Trigger::OnCalendar(format!("cron:{expr}")),
                enabled: true,
                next_run: None,
                scope, backend,
                origin: TaskOrigin::Foreign,
                lifecycle: task_scheduler_core::Lifecycle::default(),
                has_snapshot_previous: false,
                has_snapshot_original: false,
            });
        }
        i += 1;
    }
    tasks
}

fn is_cron_field(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '*' | '/' | ',' | '-'))
}


fn root_wrapper_path(name: &str) -> PathBuf {
    PathBuf::from(WRAPPER_DIR).join(format!("{name}.sh"))
}

fn write_root_wrapper(
    name: &str,
    command: &str,
    lifecycle: Lifecycle,
) -> Result<Option<PathBuf>, SchedulerError> {
    if lifecycle == Lifecycle::Persistent {
        return Ok(None);
    }
    fs::create_dir_all(WRAPPER_DIR)?;
    let path = root_wrapper_path(name);
    let cleanup = match lifecycle {
        Lifecycle::DisableAfterRun => format!(
            "awk -v n='{name}' 'BEGIN{{b=\"# >>> task-scheduler:\" n; e=\"# <<< task-scheduler:\" n; ins=0}} $0==b{{ins=1; print; next}} $0==e{{ins=0; print; next}} ins && substr($0,1,1)!=\"#\" && length($0)>0 {{print \"#DISABLED# \" $0; next}} {{print}}' /etc/crontab > /etc/crontab.tsnew && mv /etc/crontab.tsnew /etc/crontab"
        ),
        Lifecycle::DeleteAfterRun => format!(
            "awk -v n='{name}' 'BEGIN{{b=\"# >>> task-scheduler:\" n; e=\"# <<< task-scheduler:\" n; s=0}} $0==b{{s=1; next}} $0==e{{s=0; next}} !s{{print}}' /etc/crontab > /etc/crontab.tsnew && mv /etc/crontab.tsnew /etc/crontab\nrm -f '{wrapper}'",
            wrapper = path.display()
        ),
        Lifecycle::Persistent => unreachable!(),
    };
    let body = format!(
        "#!/bin/sh\n# task-scheduler wrapper for {name}\n{command}\nstatus=$?\n{cleanup}\nexit $status\n"
    );
    fs::write(&path, body)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
    Ok(Some(path))
}

fn remove_root_wrapper(name: &str) -> std::io::Result<()> {
    let p = root_wrapper_path(name);
    if p.exists() { fs::remove_file(p) } else { Ok(()) }
}
