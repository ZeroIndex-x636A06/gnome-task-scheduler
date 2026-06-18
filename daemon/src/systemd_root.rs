//! System-wide systemd adapter — writes `/etc/systemd/system/` units and,
//! for hardware-attach triggers, `/etc/udev/rules.d/` rules.
//!
//! `list_tasks` enumerates EVERYTHING in `/etc/systemd/system/`: every timer
//! we own, every foreign timer the host already had, every service that is
//! enabled at a boot target (so sunshine and friends show up), and every
//! udev-rule-backed device task we created. Each `Task` is tagged with its
//! `origin` (Owned/Foreign) so the UI can render badges and gate edits.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use task_scheduler_core::{
    device_service_name, is_owned_unit_text, parse_exec_start, parse_trigger, parse_udev_rule,
    render_service, render_timer, render_udev_rule, render_user_device_service, snapshot,
    ensure_not_protected, validate_name, Backend, Capabilities, DeviceMatch, SchedulerError,
    Scope, SnapshotTarget, Task, TaskOrigin, TaskScheduler, Trigger, UDEV_RULE_PREFIX,
    UDEV_USER_RULE_PREFIX, user_device_service_name,
};

const UNIT_DIR: &str = "/etc/systemd/system";
const UDEV_DIR: &str = "/etc/udev/rules.d";

pub struct SystemdRootAdapter {
    unit_dir: PathBuf,
    udev_dir: PathBuf,
}

impl SystemdRootAdapter {
    pub fn new() -> Self {
        Self {
            unit_dir: PathBuf::from(UNIT_DIR),
            udev_dir: PathBuf::from(UDEV_DIR),
        }
    }

    fn require_root() -> Result<(), SchedulerError> {
        let euid = unsafe { libc::geteuid() };
        if euid != 0 { return Err(SchedulerError::NotRoot); }
        Ok(())
    }

    fn service_path(&self, name: &str) -> PathBuf {
        self.unit_dir.join(format!("{name}.service"))
    }
    fn timer_path(&self, name: &str) -> PathBuf {
        self.unit_dir.join(format!("{name}.timer"))
    }
    fn device_service_path(&self, name: &str) -> PathBuf {
        self.unit_dir.join(device_service_name(name))
    }
    fn udev_rule_path(&self, name: &str) -> PathBuf {
        self.udev_dir.join(format!("{UDEV_RULE_PREFIX}{name}.rules"))
    }
    fn user_device_service_path(&self, name: &str) -> PathBuf {
        self.unit_dir.join(user_device_service_name(name))
    }
    fn udev_user_rule_path(&self, name: &str) -> PathBuf {
        self.udev_dir.join(format!("{UDEV_USER_RULE_PREFIX}{name}.rules"))
    }

    fn snapshot_root(&self) -> PathBuf { snapshot::snapshot_root(Scope::System) }

    fn systemctl(args: &[&str]) -> Result<String, SchedulerError> {
        let out = Command::new("systemctl").args(args).output()?;
        if !out.status.success() {
            return Err(SchedulerError::Systemctl {
                stderr: String::from_utf8_lossy(&out.stderr).to_string(),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    fn udevadm(args: &[&str]) -> Result<(), SchedulerError> {
        let out = Command::new("udevadm").args(args).output()?;
        if !out.status.success() {
            return Err(SchedulerError::CommandFailed(format!(
                "udevadm {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            )));
        }
        Ok(())
    }

    /// Snapshot timer + service files for `name` into `slot`. Used before
    /// any destructive edit.
    fn take_unit_snapshot(&self, name: &str, slot: &str) -> Result<(), SchedulerError> {
        let svc = self.service_path(name);
        let tmr = self.timer_path(name);
        snapshot::write_slot(
            &self.snapshot_root(),
            Backend::SystemdSystem.tag(),
            name,
            slot,
            &[svc.as_path(), tmr.as_path()],
        )
    }

    /// Snapshot a device-task's service + udev rule.
    fn take_device_snapshot(&self, name: &str, slot: &str) -> Result<(), SchedulerError> {
        let svc = self.device_service_path(name);
        let rule = self.udev_rule_path(name);
        snapshot::write_slot(
            &self.snapshot_root(),
            Backend::SystemdSystem.tag(),
            name,
            slot,
            &[svc.as_path(), rule.as_path()],
        )
    }

    fn create_device_task(&self, task: &Task, m: &DeviceMatch) -> Result<(), SchedulerError> {
        fs::create_dir_all(&self.unit_dir)?;
        fs::create_dir_all(&self.udev_dir)?;
        let svc = format!(
            "[Unit]\n\
             Description=Task Scheduler (device): {name}\n\
             \n\
             [Service]\n\
             Type=oneshot\n\
             ExecStart={command}\n",
            name = task.name,
            command = task.command,
        );
        fs::write(self.device_service_path(&task.name), svc)?;
        let service_unit = device_service_name(&task.name);
        let rule = render_udev_rule(&task.name, m, &service_unit, true);
        fs::write(self.udev_rule_path(&task.name), rule)?;
        Self::systemctl(&["daemon-reload"])?;
        Self::udevadm(&["control", "--reload"])?;
        let _ = Self::udevadm(&["trigger", &format!("--subsystem-match={}", m.subsystem)]);
        Ok(())
    }

    /// Snapshot the bridge service + udev user rule for a user-device task.
    fn take_user_device_snapshot(
        &self,
        name: &str,
        slot: &str,
        snap_root: &std::path::Path,
    ) -> Result<(), SchedulerError> {
        let svc = self.user_device_service_path(name);
        let rule = self.udev_user_rule_path(name);
        snapshot::write_slot(
            snap_root,
            Backend::SystemdUser.tag(),
            name,
            slot,
            &[svc.as_path(), rule.as_path()],
        )
    }

    /// Create a user-scope device task: write a bridge system service that
    /// runs as `username`/`uid`, plus a udev rule that activates it.
    /// Compatible with any systemd >= 183.
    pub fn create_user_device_task(
        &self,
        task: &Task,
        m: &DeviceMatch,
        username: &str,
        uid: u32,
        snap_root: &std::path::Path,
    ) -> Result<(), SchedulerError> {
        if !snapshot::has_slot(snap_root, Backend::SystemdUser.tag(), &task.name, "original")
            && (self.user_device_service_path(&task.name).exists()
                || self.udev_user_rule_path(&task.name).exists())
        {
            let _ = self.take_user_device_snapshot(&task.name, "original", snap_root);
        }
        if self.user_device_service_path(&task.name).exists()
            || self.udev_user_rule_path(&task.name).exists()
        {
            let _ = self.take_user_device_snapshot(&task.name, "previous", snap_root);
        }

        fs::create_dir_all(&self.unit_dir)?;
        fs::create_dir_all(&self.udev_dir)?;
        let svc = render_user_device_service(&task.name, &task.command, username, uid);
        fs::write(self.user_device_service_path(&task.name), svc)?;
        let service_unit = user_device_service_name(&task.name);
        let rule = render_udev_rule(&task.name, m, &service_unit, true);
        fs::write(self.udev_user_rule_path(&task.name), rule)?;
        Self::systemctl(&["daemon-reload"])?;
        Self::udevadm(&["control", "--reload"])?;
        let _ = Self::udevadm(&["trigger", &format!("--subsystem-match={}", m.subsystem)]);
        Ok(())
    }

    /// Delete a user-scope device task: remove the bridge service + udev rule.
    pub fn delete_user_device_task(
        &self,
        name: &str,
        snap_root: &std::path::Path,
    ) -> Result<(), SchedulerError> {
        let _ = self.take_user_device_snapshot(name, "previous", snap_root);
        let rule = self.udev_user_rule_path(name);
        if rule.exists() {
            fs::remove_file(&rule)?;
            let _ = Self::udevadm(&["control", "--reload"]);
        }
        let svc = self.user_device_service_path(name);
        if svc.exists() { fs::remove_file(&svc)?; }
        Self::systemctl(&["daemon-reload"])?;
        Ok(())
    }

    /// Enable or disable a user-scope device task by rewriting its udev rule.
    pub fn toggle_user_device_task(
        &self,
        name: &str,
        enable: bool,
    ) -> Result<(), SchedulerError> {
        let rule_path = self.udev_user_rule_path(name);
        let text = fs::read_to_string(&rule_path)?;
        let (parsed_name, m, _) = parse_udev_rule(&text).ok_or_else(|| {
            SchedulerError::Parse(format!("could not parse user device rule for {name}"))
        })?;
        let svc = user_device_service_name(&parsed_name);
        let new_text = render_udev_rule(&parsed_name, &m, &svc, enable);
        fs::write(&rule_path, new_text)?;
        let _ = Self::udevadm(&["control", "--reload"]);
        Ok(())
    }

    /// Revert a user-scope device task from a snapshot slot.
    pub fn revert_user_device_task(
        &self,
        name: &str,
        target: SnapshotTarget,
        snap_root: &std::path::Path,
    ) -> Result<(), SchedulerError> {
        let backend_tag = Backend::SystemdUser.tag();
        let slot = target.as_str();
        let snap_dir = snapshot::slot_dir(snap_root, backend_tag, name, slot);
        if !snap_dir.is_dir() { return Err(SchedulerError::NoSnapshot); }

        let _ = fs::remove_file(self.udev_user_rule_path(name));
        let _ = fs::remove_file(self.user_device_service_path(name));

        for entry in fs::read_dir(&snap_dir)? {
            let entry = entry?;
            let fname = entry.file_name().to_string_lossy().to_string();
            let bytes = fs::read(entry.path())?;
            let dest = if fname.ends_with(".rules") {
                self.udev_dir.join(&fname)
            } else {
                self.unit_dir.join(&fname)
            };
            fs::create_dir_all(dest.parent().unwrap())?;
            fs::write(dest, bytes)?;
        }
        Self::systemctl(&["daemon-reload"])?;
        let _ = Self::udevadm(&["control", "--reload"]);

        if matches!(target, SnapshotTarget::Original) {
            let _ = snapshot::purge_task_snapshots(snap_root, backend_tag, name);
        }
        Ok(())
    }
}

impl Default for SystemdRootAdapter {
    fn default() -> Self { Self::new() }
}

impl TaskScheduler for SystemdRootAdapter {
    fn backend(&self) -> Backend { Backend::SystemdSystem }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            on_calendar: true,
            on_boot: true,
            on_login: false,
            on_device: true,
            enable_toggle: true,
            system_scope: true,
        }
    }

    fn create_task(&self, task: &Task) -> Result<(), SchedulerError> {
        Self::require_root()?;
        validate_name(&task.name)?;
        ensure_not_protected(&task.name)?;
        if matches!(task.trigger, Trigger::OnLogin) {
            return Err(SchedulerError::UnsupportedTrigger("OnLogin".into()));
        }

        // Snapshot whatever exists at this name BEFORE the write. If we've
        // never seen this name, also record an `original` snapshot — that
        // way a user editing a pre-existing foreign unit can revert later.
        let backend_tag = Backend::SystemdSystem.tag();
        let snap_root = self.snapshot_root();
        let is_device = matches!(task.trigger, Trigger::OnDevice(_));
        if is_device {
            if !snapshot::has_slot(&snap_root, backend_tag, &task.name, "original")
                && (self.device_service_path(&task.name).exists()
                    || self.udev_rule_path(&task.name).exists())
            {
                let _ = self.take_device_snapshot(&task.name, "original");
            }
            if self.device_service_path(&task.name).exists()
                || self.udev_rule_path(&task.name).exists()
            {
                let _ = self.take_device_snapshot(&task.name, "previous");
            }
        } else {
            if !snapshot::has_slot(&snap_root, backend_tag, &task.name, "original")
                && (self.service_path(&task.name).exists()
                    || self.timer_path(&task.name).exists())
            {
                let _ = self.take_unit_snapshot(&task.name, "original");
            }
            if self.service_path(&task.name).exists()
                || self.timer_path(&task.name).exists()
            {
                let _ = self.take_unit_snapshot(&task.name, "previous");
            }
        }

        if let Trigger::OnDevice(m) = &task.trigger {
            return self.create_device_task(task, m);
        }
        fs::create_dir_all(&self.unit_dir)?;
        fs::write(self.service_path(&task.name), render_service(task))?;
        fs::write(self.timer_path(&task.name), render_timer(task))?;
        Self::systemctl(&["daemon-reload"])?;
        Self::systemctl(&["enable", "--now", &format!("{}.timer", task.name)])?;
        Ok(())
    }

    fn delete_task(&self, name: &str) -> Result<(), SchedulerError> {
        Self::require_root()?;
        validate_name(name)?;
        ensure_not_protected(name)?;
        // Snapshot before destructive delete so revert-to-previous still works.
        let _ = self.take_unit_snapshot(name, "previous");
        let _ = self.take_device_snapshot(name, "previous");

        let _ = Self::systemctl(&["disable", "--now", &format!("{name}.timer")]);
        let svc = self.service_path(name);
        let tmr = self.timer_path(name);
        if svc.exists() { fs::remove_file(&svc)?; }
        if tmr.exists() { fs::remove_file(&tmr)?; }
        let dsvc = self.device_service_path(name);
        let rule = self.udev_rule_path(name);
        if rule.exists() {
            fs::remove_file(&rule)?;
            let _ = Self::udevadm(&["control", "--reload"]);
        }
        if dsvc.exists() { fs::remove_file(&dsvc)?; }
        Self::systemctl(&["daemon-reload"])?;
        Ok(())
    }

    fn toggle_task(&self, name: &str, enable: bool) -> Result<(), SchedulerError> {
        Self::require_root()?;
        validate_name(name)?;
        ensure_not_protected(name)?;
        let rule = self.udev_rule_path(name);
        if rule.exists() {
            let text = fs::read_to_string(&rule)?;
            let (parsed_name, m, _) = parse_udev_rule(&text).ok_or_else(|| {
                SchedulerError::Parse(format!("could not parse udev rule for {name}"))
            })?;
            let svc = device_service_name(&parsed_name);
            let new_text = render_udev_rule(&parsed_name, &m, &svc, enable);
            fs::write(&rule, new_text)?;
            let _ = Self::udevadm(&["control", "--reload"]);
            return Ok(());
        }
        // For both owned and foreign timers/services, systemctl can toggle.
        let verb = if enable { "enable" } else { "disable" };
        // Try .timer first; fall back to .service for boot-enabled units.
        let timer_unit = format!("{name}.timer");
        if self.timer_path(name).exists() {
            Self::systemctl(&[verb, "--now", &timer_unit])?;
        } else {
            Self::systemctl(&[verb, "--now", &format!("{name}.service")])?;
        }
        Ok(())
    }

    fn revert_task(
        &self,
        name: &str,
        target: SnapshotTarget,
    ) -> Result<(), SchedulerError> {
        Self::require_root()?;
        validate_name(name)?;
        ensure_not_protected(name)?;
        let backend_tag = Backend::SystemdSystem.tag();
        let snap_root = self.snapshot_root();
        let slot = target.as_str();

        // Detect whether the snapshot is a device-style task (has a .rules
        // file) or a unit-style task.
        let snap_files: Vec<String> = {
            let dir = snapshot::slot_dir(&snap_root, backend_tag, name, slot);
            if !dir.is_dir() { return Err(SchedulerError::NoSnapshot); }
            fs::read_dir(&dir)?
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect()
        };
        let is_device = snap_files.iter().any(|f| f.ends_with(".rules"));

        if is_device {
            // Remove current files, restore snapshot, reload udev.
            let _ = fs::remove_file(self.device_service_path(name));
            let _ = fs::remove_file(self.udev_rule_path(name));
            // Restore into the appropriate dirs by file name.
            let src = snapshot::slot_dir(&snap_root, backend_tag, name, slot);
            for entry in fs::read_dir(&src)? {
                let entry = entry?;
                let fname = entry.file_name();
                let fname_str = fname.to_string_lossy().to_string();
                let bytes = fs::read(entry.path())?;
                let dest = if fname_str.ends_with(".rules") {
                    self.udev_dir.join(&fname_str)
                } else {
                    self.unit_dir.join(&fname_str)
                };
                fs::create_dir_all(dest.parent().unwrap())?;
                fs::write(dest, bytes)?;
            }
            Self::systemctl(&["daemon-reload"])?;
            let _ = Self::udevadm(&["control", "--reload"]);
        } else {
            // Unit-style — remove current .service / .timer first.
            let _ = Self::systemctl(&["disable", "--now", &format!("{name}.timer")]);
            let _ = fs::remove_file(self.service_path(name));
            let _ = fs::remove_file(self.timer_path(name));
            snapshot::restore_slot(&snap_root, backend_tag, name, slot, &self.unit_dir)?;
            Self::systemctl(&["daemon-reload"])?;
            // Best-effort re-enable if a .timer was restored.
            if self.timer_path(name).exists() {
                let _ = Self::systemctl(&["enable", "--now", &format!("{name}.timer")]);
            }
        }

        // If we just reverted to the original (pre-Task-Scheduler) state,
        // throw away snapshots so the task looks pristine again.
        if matches!(target, SnapshotTarget::Original) {
            let _ = snapshot::purge_task_snapshots(&snap_root, backend_tag, name);
        }
        Ok(())
    }

    fn list_tasks(&self) -> Result<Vec<Task>, SchedulerError> {
        let mut tasks = Vec::new();
        let backend_tag = Backend::SystemdSystem.tag();
        let snap_root = self.snapshot_root();

        let next_runs: std::collections::HashMap<String, String> =
            Self::systemctl(&["list-timers", "--all", "--output=json"])
                .ok()
                .and_then(|out| serde_json::from_str::<serde_json::Value>(&out).ok())
                .and_then(|v| v.as_array().cloned())
                .map(|arr| {
                    arr.into_iter()
                        .filter_map(|e| {
                            let u = e.get("unit")?.as_str()?.to_string();
                            let n = e
                                .get("next")
                                .and_then(|n| n.as_str().map(str::to_string))
                                .unwrap_or_default();
                            Some((u, n))
                        })
                        .collect()
                })
                .unwrap_or_default();

        let mut paired_services: HashSet<String> = HashSet::new();

        if self.unit_dir.exists() {
            // Pass 1: every .timer + paired .service (owned or foreign).
            for entry in fs::read_dir(&self.unit_dir)? {
                let entry = entry?;
                let path = entry.path();
                let Some(name_os) = path.file_name() else { continue; };
                let fname = name_os.to_string_lossy().to_string();
                let Some(stem) = fname.strip_suffix(".timer") else { continue; };
                let timer_text = fs::read_to_string(&path).unwrap_or_default();
                let service_text =
                    fs::read_to_string(self.service_path(stem)).unwrap_or_default();
                paired_services.insert(stem.to_string());
                let origin = if is_owned_unit_text(&service_text) || is_owned_unit_text(&timer_text) {
                    TaskOrigin::Owned
                } else {
                    TaskOrigin::Foreign
                };
                let trigger = parse_trigger(&timer_text)
                    .unwrap_or(Trigger::OnCalendar("(unknown)".into()));
                let command = parse_exec_start(&service_text).unwrap_or_default();
                let timer_unit = format!("{stem}.timer");
                let enabled = Self::systemctl(&["is-enabled", &timer_unit])
                    .map(|s| s.trim() == "enabled")
                    .unwrap_or(false);
                tasks.push(Task {
                    name: stem.to_string(),
                    command,
                    trigger,
                    enabled,
                    next_run: next_runs.get(&timer_unit).cloned(),
                    scope: Scope::System,
                    backend: Backend::SystemdSystem,
                    origin,
                    lifecycle: task_scheduler_core::Lifecycle::default(),
                    has_snapshot_previous: snapshot::has_slot(&snap_root, backend_tag, stem, "previous"),
                    has_snapshot_original: snapshot::has_slot(&snap_root, backend_tag, stem, "original"),
                });
            }

            // Pass 2: services without a paired timer that are enabled at a
            // boot target. Sunshine lands here.
            for entry in fs::read_dir(&self.unit_dir)? {
                let entry = entry?;
                let path = entry.path();
                let Some(name_os) = path.file_name() else { continue; };
                let fname = name_os.to_string_lossy().to_string();
                let Some(stem) = fname.strip_suffix(".service") else { continue; };
                // Skip our own device-trigger services — they show up via the
                // udev-rule path below.
                if stem.starts_with("task-scheduler-device-") { continue; }
                if paired_services.contains(stem) { continue; }
                let text = fs::read_to_string(&path).unwrap_or_default();
                if !service_is_boot_enabled(&text) { continue; }
                let command = parse_exec_start(&text).unwrap_or_default();
                let svc_unit = format!("{stem}.service");
                let enabled = Self::systemctl(&["is-enabled", &svc_unit])
                    .map(|s| {
                        let s = s.trim();
                        s == "enabled" || s == "enabled-runtime" || s == "alias"
                    })
                    .unwrap_or(false);
                tasks.push(Task {
                    name: stem.to_string(),
                    command,
                    trigger: Trigger::OnBootSec("at boot".into()),
                    enabled,
                    next_run: None,
                    scope: Scope::System,
                    backend: Backend::SystemdSystem,
                    origin: if is_owned_unit_text(&text) {
                        TaskOrigin::Owned
                    } else {
                        TaskOrigin::Foreign
                    },
                    lifecycle: task_scheduler_core::Lifecycle::default(),
                    has_snapshot_previous: snapshot::has_slot(&snap_root, backend_tag, stem, "previous"),
                    has_snapshot_original: snapshot::has_slot(&snap_root, backend_tag, stem, "original"),
                });
            }
        }

        // Device-trigger tasks (always owned — we're the only thing that
        // writes 90-task-scheduler-*.rules).
        if self.udev_dir.exists() {
            for entry in fs::read_dir(&self.udev_dir)? {
                let entry = entry?;
                let path = entry.path();
                let Some(name_os) = path.file_name() else { continue; };
                let fname = name_os.to_string_lossy().to_string();
                let Some(rest) = fname.strip_prefix(UDEV_RULE_PREFIX) else { continue };
                // User-device rules share the common prefix — they're listed by
                // SystemdUserAdapter, so skip them here to avoid double-listing.
                if rest.starts_with("user-") { continue; }
                let Some(stem) = rest.strip_suffix(".rules") else { continue };
                let text = fs::read_to_string(&path).unwrap_or_default();
                let Some((name, m, enabled)) = parse_udev_rule(&text) else { continue };
                let service_path = self.device_service_path(&name);
                let command = fs::read_to_string(&service_path)
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
                    scope: Scope::System,
                    backend: Backend::SystemdSystem,
                    origin: TaskOrigin::Owned,
                    lifecycle: task_scheduler_core::Lifecycle::default(),
                    has_snapshot_previous: snapshot::has_slot(&snap_root, backend_tag, &name, "previous"),
                    has_snapshot_original: snapshot::has_slot(&snap_root, backend_tag, &name, "original"),
                });
            }
        }

        tasks.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(tasks)
    }
}

/// Quick check: does this systemd `.service` body have `[Install] WantedBy=`
/// a typical boot target? Used to surface services like sunshine that are
/// triggered "at boot" but have no `.timer`.
fn service_is_boot_enabled(text: &str) -> bool {
    let mut in_install = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_install = t.eq_ignore_ascii_case("[Install]");
            continue;
        }
        if !in_install { continue; }
        if let Some(rest) = t.strip_prefix("WantedBy=") {
            for tgt in rest.split(|c: char| c.is_whitespace() || c == ',') {
                match tgt.trim() {
                    "multi-user.target"
                    | "default.target"
                    | "graphical.target"
                    | "basic.target" => return true,
                    _ => {}
                }
            }
        }
        // Some distro services use `RequiredBy=` instead.
        if let Some(rest) = t.strip_prefix("RequiredBy=") {
            for tgt in rest.split(|c: char| c.is_whitespace() || c == ',') {
                match tgt.trim() {
                    "multi-user.target" | "default.target" | "graphical.target" => return true,
                    _ => {}
                }
            }
        }
    }
    false
}


