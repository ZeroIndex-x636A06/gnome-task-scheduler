//! Per-user cron via the `crontab` CLI.
//!
//! `list_tasks` enumerates BOTH our marker-wrapped task blocks (Owned) and
//! every other active crontab line (Foreign), so the user sees their whole
//! crontab in the task library.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use task_scheduler_core::{
    snapshot, systemd_calendar_to_cron, ensure_not_protected, validate_name, Backend, Capabilities, Lifecycle,
    SchedulerError, Scope, SnapshotTarget, Task, TaskOrigin, TaskScheduler, Trigger,
};


const MARKER_BEGIN: &str = "# >>> task-scheduler:";
const MARKER_END: &str = "# <<< task-scheduler:";
const SNAPSHOT_FILE: &str = "crontab.txt";

pub struct CronUserAdapter;

impl CronUserAdapter {
    pub fn new() -> Self { Self }

    fn read() -> Result<String, SchedulerError> {
        let out = Command::new("crontab").arg("-l").output()?;
        if !out.status.success() && !out.stderr.is_empty() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !stderr.contains("no crontab") {
                // tolerate
            }
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    fn write(contents: &str) -> Result<(), SchedulerError> {
        let mut child = Command::new("crontab")
            .arg("-")
            .stdin(Stdio::piped())
            .spawn()?;
        child
            .stdin
            .as_mut()
            .ok_or_else(|| SchedulerError::CommandFailed("crontab stdin".into()))?
            .write_all(contents.as_bytes())?;
        let status = child.wait()?;
        if !status.success() {
            return Err(SchedulerError::CommandFailed("crontab - failed".into()));
        }
        Ok(())
    }

    fn snapshot_root() -> std::path::PathBuf { snapshot::snapshot_root(Scope::User) }

    fn record_snapshot(slot: &str) -> Result<(), SchedulerError> {
        let body = Self::read().unwrap_or_default();
        snapshot::write_slot_inline(
            &Self::snapshot_root(),
            Backend::CronUser.tag(),
            "_all",
            slot,
            SNAPSHOT_FILE,
            &body,
        )
    }
}

impl Default for CronUserAdapter { fn default() -> Self { Self::new() } }

impl TaskScheduler for CronUserAdapter {
    fn backend(&self) -> Backend { Backend::CronUser }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            on_calendar: true,
            on_boot: false,
            on_login: false,
            on_device: false,
            enable_toggle: true,
            system_scope: false,
        }
    }

    fn create_task(&self, task: &Task) -> Result<(), SchedulerError> {
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
        // Snapshot full crontab once as `original`, and always as `previous`.
        let snap_root = Self::snapshot_root();
        if !snapshot::has_slot(&snap_root, Backend::CronUser.tag(), "_all", "original") {
            let _ = Self::record_snapshot("original");
        }
        let _ = Self::record_snapshot("previous");

        // For one-shot lifecycles, install a wrapper script and run that
        // instead of the bare command so cleanup runs after the user's task.
        let cmd_field = match write_user_wrapper(&task.name, &task.command, task.lifecycle)? {
            Some(p) => format!("/bin/sh {}", p.display()),
            None => task.command.clone(),
        };

        let mut text = strip_block(&Self::read()?, &task.name);
        text.push_str(&format!(
            "{begin}{name}\n{cron} {cmd}\n{end}{name}\n",
            begin = MARKER_BEGIN, end = MARKER_END,
            name = task.name, cron = cron, cmd = cmd_field,
        ));
        Self::write(&text)
    }

    fn delete_task(&self, name: &str) -> Result<(), SchedulerError> {
        validate_name(name)?;
        ensure_not_protected(name)?;
        let _ = Self::record_snapshot("previous");
        let text = strip_block(&Self::read()?, name);
        let _ = remove_user_wrapper(name);
        Self::write(&text)
    }


    fn toggle_task(&self, name: &str, enable: bool) -> Result<(), SchedulerError> {
        validate_name(name)?;
        ensure_not_protected(name)?;
        let _ = Self::record_snapshot("previous");
        let text = toggle_block(&Self::read()?, name, enable);
        Self::write(&text)
    }

    fn revert_task(&self, _name: &str, target: SnapshotTarget) -> Result<(), SchedulerError> {
        // Cron snapshots are whole-crontab snapshots; revert restores the
        // whole crontab. `name` is ignored.
        let snap_root = Self::snapshot_root();
        let body = snapshot::restore_slot_inline(
            &snap_root,
            Backend::CronUser.tag(),
            "_all",
            target.as_str(),
            SNAPSHOT_FILE,
        )?;
        Self::write(&body)?;
        if matches!(target, SnapshotTarget::Original) {
            let _ = snapshot::purge_task_snapshots(&snap_root, Backend::CronUser.tag(), "_all");
        }
        Ok(())
    }

    fn list_tasks(&self) -> Result<Vec<Task>, SchedulerError> {
        let text = Self::read().unwrap_or_default();
        let snap_root = Self::snapshot_root();
        let has_prev = snapshot::has_slot(&snap_root, Backend::CronUser.tag(), "_all", "previous");
        let has_orig = snapshot::has_slot(&snap_root, Backend::CronUser.tag(), "_all", "original");
        let mut all = parse_all(&text, Scope::User, Backend::CronUser);
        for t in all.iter_mut() {
            t.has_snapshot_previous = has_prev;
            t.has_snapshot_original = has_orig;
        }
        Ok(all)
    }
}

fn strip_block(text: &str, name: &str) -> String {
    let begin = format!("{MARKER_BEGIN}{name}");
    let end = format!("{MARKER_END}{name}");
    let mut out = String::with_capacity(text.len());
    let mut inside = false;
    for line in text.lines() {
        if line.starts_with(&begin) { inside = true; continue; }
        if inside && line.starts_with(&end) { inside = false; continue; }
        if !inside { out.push_str(line); out.push('\n'); }
    }
    out
}

fn toggle_block(text: &str, name: &str, enable: bool) -> String {
    let begin = format!("{MARKER_BEGIN}{name}");
    let end = format!("{MARKER_END}{name}");
    let mut out = String::with_capacity(text.len());
    let mut inside = false;
    for line in text.lines() {
        if line.starts_with(&begin) { inside = true; out.push_str(line); out.push('\n'); continue; }
        if inside && line.starts_with(&end) { inside = false; out.push_str(line); out.push('\n'); continue; }
        if inside {
            let t = line.trim_start();
            if let Some(rest) = t.strip_prefix("#DISABLED# ") {
                if enable { out.push_str(rest); out.push('\n'); }
                else { out.push_str(line); out.push('\n'); }
            } else if !t.is_empty() && !t.starts_with('#') {
                if enable { out.push_str(line); out.push('\n'); }
                else { out.push_str("#DISABLED# "); out.push_str(line); out.push('\n'); }
            } else {
                out.push_str(line); out.push('\n');
            }
        } else {
            out.push_str(line); out.push('\n');
        }
    }
    out
}

/// Parse owned (marker-wrapped) blocks AND foreign cron lines into Tasks.
/// Foreign tasks get auto-generated names `cron-<n>` so each row is unique.
fn parse_all(text: &str, scope: Scope, backend: Backend) -> Vec<Task> {
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
                let parts: Vec<&str> = body.splitn(6, char::is_whitespace).collect();
                if parts.len() >= 6 {
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
        // Foreign line — try to parse as a cron entry.
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            i += 1; continue;
        }
        let parts: Vec<&str> = trimmed.splitn(6, char::is_whitespace).collect();
        if parts.len() >= 6 && is_cron_field(parts[0]) {
            let expr = parts[..5].join(" ");
            let cmd = parts[5].to_string();
            foreign_counter += 1;
            tasks.push(Task {
                name: format!("cron-{foreign_counter}"),
                command: cmd,
                trigger: Trigger::OnCalendar(format!("cron:{expr}")),
                enabled: true,
                next_run: None,
                scope,
                backend,
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

fn user_wrapper_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("task-scheduler").join("wrappers"))
}

fn user_wrapper_path(name: &str) -> Option<PathBuf> {
    user_wrapper_dir().map(|d| d.join(format!("{name}.sh")))
}

/// Writes the lifecycle wrapper script for a user-cron task.
/// Returns `Ok(None)` for `Persistent` (no wrapper needed).
pub(crate) fn write_user_wrapper(
    name: &str,
    command: &str,
    lifecycle: Lifecycle,
) -> Result<Option<PathBuf>, SchedulerError> {
    if lifecycle == Lifecycle::Persistent {
        return Ok(None);
    }
    let dir = user_wrapper_dir().ok_or(SchedulerError::NoConfigDir)?;
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{name}.sh"));
    let cleanup = match lifecycle {
        Lifecycle::DisableAfterRun => format!(
            "crontab -l 2>/dev/null | awk -v n='{name}' 'BEGIN{{b=\"# >>> task-scheduler:\" n; e=\"# <<< task-scheduler:\" n; ins=0}} $0==b{{ins=1; print; next}} $0==e{{ins=0; print; next}} ins && substr($0,1,1)!=\"#\" && length($0)>0 {{print \"#DISABLED# \" $0; next}} {{print}}' | crontab -"
        ),
        Lifecycle::DeleteAfterRun => format!(
            "crontab -l 2>/dev/null | awk -v n='{name}' 'BEGIN{{b=\"# >>> task-scheduler:\" n; e=\"# <<< task-scheduler:\" n; s=0}} $0==b{{s=1; next}} $0==e{{s=0; next}} !s{{print}}' | crontab -\nrm -f '{wrapper}'",
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

pub(crate) fn remove_user_wrapper(name: &str) -> std::io::Result<()> {
    if let Some(p) = user_wrapper_path(name) {
        if p.exists() {
            return fs::remove_file(p);
        }
    }
    Ok(())
}
