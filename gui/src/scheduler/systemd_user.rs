//! User-scope systemd via `systemctl --user`.
//!
//! `list_tasks` enumerates every `.timer` + paired `.service` in
//! `~/.config/systemd/user/`, plus every `.service` enabled at a user-mode
//! boot target without a `.timer`. Each task is tagged with `origin` so the
//! UI can mark pre-existing units as "system" and gate edits.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use task_scheduler_core::{
    is_owned_unit_text, parse_exec_start, parse_trigger, parse_udev_rule, render_service,
    render_timer, snapshot, ensure_not_protected, validate_name, Backend, Capabilities,
    Lifecycle, SchedulerError, Scope, SnapshotTarget, Task, TaskOrigin, TaskScheduler, Trigger,
    UDEV_USER_RULE_PREFIX, user_device_service_name,
};

use crate::scheduler::systemd_root_proxy::UserDeviceBridge;

const UDEV_DIR: &str = "/etc/udev/rules.d";
const SYSTEM_UNIT_DIR: &str = "/etc/systemd/system";

pub struct SystemdUserAdapter {
    unit_dir: PathBuf,
    /// Present only when the privileged daemon is reachable.
    bridge: Option<UserDeviceBridge>,
}

impl SystemdUserAdapter {
    pub fn new(daemon_available: bool) -> Self {
        let uid = unsafe { libc::getuid() };
        let unit_dir = dirs::config_dir()
            .map(|d| d.join("systemd").join("user"))
            .unwrap_or_else(|| PathBuf::from("/tmp/systemd-user-fallback"));
        Self {
            unit_dir,
            bridge: if daemon_available { Some(UserDeviceBridge::new(uid)) } else { None },
        }
    }

    fn service_path(&self, n: &str) -> PathBuf { self.unit_dir.join(format!("{n}.service")) }
    fn timer_path(&self, n: &str) -> PathBuf { self.unit_dir.join(format!("{n}.timer")) }
    fn snapshot_root(&self) -> PathBuf { snapshot::snapshot_root(Scope::User) }
    fn udev_user_rule_path(name: &str) -> PathBuf {
        std::path::Path::new(UDEV_DIR).join(format!("{UDEV_USER_RULE_PREFIX}{name}.rules"))
    }

    fn systemctl(args: &[&str]) -> Result<String, SchedulerError> {
        let out = Command::new("systemctl").args(args).output()?;
        if !out.status.success() {
            return Err(SchedulerError::Systemctl {
                stderr: String::from_utf8_lossy(&out.stderr).to_string(),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    fn take_snapshot(&self, name: &str, slot: &str) -> Result<(), SchedulerError> {
        let svc = self.service_path(name);
        let tmr = self.timer_path(name);
        snapshot::write_slot(
            &self.snapshot_root(),
            Backend::SystemdUser.tag(),
            name,
            slot,
            &[svc.as_path(), tmr.as_path()],
        )
    }
}

impl Default for SystemdUserAdapter {
    fn default() -> Self { Self::new(false) }
}

impl TaskScheduler for SystemdUserAdapter {
    fn backend(&self) -> Backend { Backend::SystemdUser }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            on_calendar: true,
            on_boot: true,
            on_login: false,
            on_device: self.bridge.is_some(),
            enable_toggle: true,
            system_scope: false,
        }
    }

    fn create_task(&self, task: &Task) -> Result<(), SchedulerError> {
        validate_name(&task.name)?;
        ensure_not_protected(&task.name)?;

        // Device triggers need root (udev rule + bridge service) — delegate to daemon.
        if let Trigger::OnDevice(ref m) = task.trigger {
            let bridge = self.bridge.as_ref().ok_or_else(|| {
                SchedulerError::UnsupportedTrigger(
                    "device triggers require the Task Scheduler daemon".into(),
                )
            })?;
            return bridge.create(
                &task.name,
                &task.command,
                &serde_json::to_string(m).unwrap_or_default(),
            );
        }

        if matches!(task.trigger, Trigger::OnLogin) {
            return Err(SchedulerError::UnsupportedTrigger(task.trigger.kind().into()));
        }
        // Snapshot before clobbering. Record `original` once.
        let snap_root = self.snapshot_root();
        let backend_tag = Backend::SystemdUser.tag();
        let exists = self.service_path(&task.name).exists() || self.timer_path(&task.name).exists();
        if exists && !snapshot::has_slot(&snap_root, backend_tag, &task.name, "original") {
            let _ = self.take_snapshot(&task.name, "original");
        }
        if exists {
            let _ = self.take_snapshot(&task.name, "previous");
        }

        fs::create_dir_all(&self.unit_dir)?;
        fs::write(self.service_path(&task.name), render_service(task))?;
        fs::write(self.timer_path(&task.name), render_timer(task))?;
        Self::systemctl(&["--user", "daemon-reload"])?;
        Self::systemctl(&["--user", "enable", "--now", &format!("{}.timer", task.name)])?;
        Ok(())
    }

    fn delete_task(&self, name: &str) -> Result<(), SchedulerError> {
        validate_name(name)?;
        ensure_not_protected(name)?;
        // User device task: delegate to daemon (it owns the udev rule + bridge service).
        if Self::udev_user_rule_path(name).exists() {
            return self.bridge.as_ref()
                .ok_or_else(|| SchedulerError::UnsupportedTrigger(
                    "daemon required to remove device trigger".into(),
                ))?
                .delete(name);
        }
        let _ = self.take_snapshot(name, "previous");
        let _ = Self::systemctl(&["--user", "disable", "--now", &format!("{name}.timer")]);
        let svc = self.service_path(name);
        let tmr = self.timer_path(name);
        if svc.exists() { fs::remove_file(&svc)?; }
        if tmr.exists() { fs::remove_file(&tmr)?; }
        Self::systemctl(&["--user", "daemon-reload"])?;
        Ok(())
    }

    fn toggle_task(&self, name: &str, enable: bool) -> Result<(), SchedulerError> {
        validate_name(name)?;
        ensure_not_protected(name)?;
        // User device task: daemon toggles the udev rule.
        if Self::udev_user_rule_path(name).exists() {
            return self.bridge.as_ref()
                .ok_or_else(|| SchedulerError::UnsupportedTrigger(
                    "daemon required to toggle device trigger".into(),
                ))?
                .toggle(name, enable);
        }
        let verb = if enable { "enable" } else { "disable" };
        if self.timer_path(name).exists() {
            Self::systemctl(&["--user", verb, "--now", &format!("{name}.timer")])?;
        } else {
            Self::systemctl(&["--user", verb, "--now", &format!("{name}.service")])?;
        }
        Ok(())
    }

    fn revert_task(&self, name: &str, target: SnapshotTarget) -> Result<(), SchedulerError> {
        validate_name(name)?;
        ensure_not_protected(name)?;
        // User device task: daemon handles the revert (files live in system dirs).
        if Self::udev_user_rule_path(name).exists() {
            return self.bridge.as_ref()
                .ok_or_else(|| SchedulerError::UnsupportedTrigger(
                    "daemon required to revert device trigger".into(),
                ))?
                .revert(name, target);
        }
        let backend_tag = Backend::SystemdUser.tag();
        let snap_root = self.snapshot_root();
        let _ = Self::systemctl(&["--user", "disable", "--now", &format!("{name}.timer")]);
        let _ = fs::remove_file(self.service_path(name));
        let _ = fs::remove_file(self.timer_path(name));
        snapshot::restore_slot(&snap_root, backend_tag, name, target.as_str(), &self.unit_dir)?;
        Self::systemctl(&["--user", "daemon-reload"])?;
        if self.timer_path(name).exists() {
            let _ = Self::systemctl(&["--user", "enable", "--now", &format!("{name}.timer")]);
        }
        if matches!(target, SnapshotTarget::Original) {
            let _ = snapshot::purge_task_snapshots(&snap_root, backend_tag, name);
        }
        Ok(())
    }

    fn list_tasks(&self) -> Result<Vec<Task>, SchedulerError> {
        if !self.unit_dir.exists() { return Ok(vec![]); }
        let backend_tag = Backend::SystemdUser.tag();
        let snap_root = self.snapshot_root();
        let next_runs: std::collections::HashMap<String, String> =
            Self::systemctl(&["--user", "list-timers", "--all", "--output=json"])
                .ok()
                .and_then(|out| serde_json::from_str::<serde_json::Value>(&out).ok())
                .and_then(|v| v.as_array().cloned())
                .map(|arr| arr.into_iter().filter_map(|e| {
                    let u = e.get("unit")?.as_str()?.to_string();
                    let n = e.get("next").and_then(|n| n.as_str().map(str::to_string)).unwrap_or_default();
                    Some((u, n))
                }).collect())
                .unwrap_or_default();

        let mut tasks = Vec::new();
        let mut paired: HashSet<String> = HashSet::new();
        for entry in fs::read_dir(&self.unit_dir)? {
            let entry = entry?;
            let path = entry.path();
            let Some(name_os) = path.file_name() else { continue; };
            let fname = name_os.to_string_lossy().to_string();
            let Some(stem) = fname.strip_suffix(".timer") else { continue; };
            paired.insert(stem.to_string());
            let timer_text = fs::read_to_string(&path).unwrap_or_default();
            let service_text = fs::read_to_string(self.service_path(stem)).unwrap_or_default();
            let trigger = parse_trigger(&timer_text).unwrap_or(Trigger::OnCalendar("(unknown)".into()));
            let command = parse_exec_start(&service_text).unwrap_or_default();
            let timer_unit = format!("{stem}.timer");
            let enabled = Self::systemctl(&["--user", "is-enabled", &timer_unit])
                .map(|s| s.trim() == "enabled").unwrap_or(false);
            let origin = if is_owned_unit_text(&service_text) || is_owned_unit_text(&timer_text) {
                TaskOrigin::Owned
            } else {
                TaskOrigin::Foreign
            };
            tasks.push(Task {
                name: stem.to_string(),
                command, trigger, enabled,
                next_run: next_runs.get(&timer_unit).cloned(),
                scope: Scope::User,
                backend: Backend::SystemdUser,
                origin,
                lifecycle: task_scheduler_core::Lifecycle::default(),
                has_snapshot_previous: snapshot::has_slot(&snap_root, backend_tag, stem, "previous"),
                has_snapshot_original: snapshot::has_slot(&snap_root, backend_tag, stem, "original"),
            });
        }
        // Boot-enabled services without a timer (rare for --user, but surface them).
        for entry in fs::read_dir(&self.unit_dir)? {
            let entry = entry?;
            let path = entry.path();
            let Some(name_os) = path.file_name() else { continue; };
            let fname = name_os.to_string_lossy().to_string();
            let Some(stem) = fname.strip_suffix(".service") else { continue };
            if paired.contains(stem) { continue }
            let text = fs::read_to_string(&path).unwrap_or_default();
            if !text.contains("WantedBy=default.target")
                && !text.contains("WantedBy=basic.target")
            {
                continue;
            }
            let command = parse_exec_start(&text).unwrap_or_default();
            let svc_unit = format!("{stem}.service");
            let enabled = Self::systemctl(&["--user", "is-enabled", &svc_unit])
                .map(|s| s.trim() == "enabled").unwrap_or(false);
            tasks.push(Task {
                name: stem.to_string(),
                command,
                trigger: Trigger::OnBootSec("at session start".into()),
                enabled,
                next_run: None,
                scope: Scope::User,
                backend: Backend::SystemdUser,
                origin: if is_owned_unit_text(&text) { TaskOrigin::Owned } else { TaskOrigin::Foreign },
                lifecycle: task_scheduler_core::Lifecycle::default(),
                has_snapshot_previous: snapshot::has_slot(&snap_root, backend_tag, stem, "previous"),
                has_snapshot_original: snapshot::has_slot(&snap_root, backend_tag, stem, "original"),
            });
        }
        // User-device tasks: bridge service + udev rule written by the daemon.
        // /etc/udev/rules.d is world-readable, so we can scan it directly.
        let udev_dir = std::path::Path::new(UDEV_DIR);
        if udev_dir.exists() {
            if let Ok(entries) = fs::read_dir(udev_dir) {
                for entry in entries.flatten() {
                    let fname = entry.file_name().to_string_lossy().to_string();
                    let Some(rest) = fname.strip_prefix(UDEV_USER_RULE_PREFIX) else { continue };
                    let Some(stem) = rest.strip_suffix(".rules") else { continue };
                    let text = fs::read_to_string(entry.path()).unwrap_or_default();
                    let Some((name, m, enabled)) = parse_udev_rule(&text) else { continue };
                    let svc_path = std::path::Path::new(SYSTEM_UNIT_DIR)
                        .join(user_device_service_name(&name));
                    let command = fs::read_to_string(&svc_path)
                        .ok()
                        .and_then(|s| parse_exec_start(&s))
                        .unwrap_or_default();
                    let _ = stem;
                    tasks.push(Task {
                        name: name.clone(),
                        command,
                        trigger: Trigger::OnDevice(m),
                        enabled,
                        next_run: None,
                        scope: Scope::User,
                        backend: Backend::SystemdUser,
                        origin: TaskOrigin::Owned,
                        lifecycle: Lifecycle::default(),
                        has_snapshot_previous: snapshot::has_slot(
                            &snap_root, backend_tag, &name, "previous",
                        ),
                        has_snapshot_original: snapshot::has_slot(
                            &snap_root, backend_tag, &name, "original",
                        ),
                    });
                }
            }
        }

        tasks.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(tasks)
    }
}
