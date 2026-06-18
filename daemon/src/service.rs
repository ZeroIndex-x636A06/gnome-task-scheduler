//! zbus interface — dispatches to one of several root adapters by backend tag.

use std::collections::HashMap;
use std::ffi::CStr;
use std::path::PathBuf;

use task_scheduler_core::{
    Backend, DeviceMatch, Lifecycle, SchedulerError, Scope, SnapshotTarget, Task, TaskOrigin,
    TaskScheduler, Trigger,
};


use tracing::{error, info};
use zbus::fdo;

use crate::cron_root::CronRootAdapter;
use crate::openrc_root::OpenRcRootAdapter;
use crate::systemd_root::SystemdRootAdapter;

pub struct SchedulerService {
    adapters: HashMap<Backend, Box<dyn TaskScheduler + Send + Sync>>,
    /// Direct reference for user-device operations not in the TaskScheduler trait.
    systemd_root: SystemdRootAdapter,
}

impl SchedulerService {
    pub fn new() -> Self {
        let mut adapters: HashMap<Backend, Box<dyn TaskScheduler + Send + Sync>> =
            HashMap::new();
        adapters.insert(Backend::SystemdSystem, Box::new(SystemdRootAdapter::new()));
        adapters.insert(Backend::CronSystem, Box::new(CronRootAdapter::new()));
        adapters.insert(Backend::OpenRc, Box::new(OpenRcRootAdapter::new()));
        Self { adapters, systemd_root: SystemdRootAdapter::new() }
    }

    fn adapter(&self, tag: &str) -> Result<&(dyn TaskScheduler + Send + Sync), fdo::Error> {
        let backend = Backend::from_tag(tag).map_err(to_fdo)?;
        self.adapters
            .get(&backend)
            .map(|b| b.as_ref())
            .ok_or_else(|| {
                fdo::Error::NotSupported(format!("backend not enabled: {tag}"))
            })
    }
}

#[zbus::interface(name = "org.linux.TaskScheduler")]
impl SchedulerService {
    fn create_task(
        &self,
        backend: &str,
        name: &str,
        command: &str,
        trigger_kind: &str,
        trigger_value: &str,
        lifecycle: &str,
    ) -> fdo::Result<()> {
        info!(backend, name, trigger_kind, lifecycle, "CreateTask");
        let adapter = self.adapter(backend)?;
        let trigger =
            Trigger::from_parts(trigger_kind, trigger_value).map_err(to_fdo)?;
        let task = Task {
            name: name.to_string(),
            command: command.to_string(),
            trigger,
            enabled: true,
            next_run: None,
            scope: Scope::System,
            backend: Backend::from_tag(backend).map_err(to_fdo)?,
            origin: TaskOrigin::Owned,
            lifecycle: Lifecycle::from_str(lifecycle),
            has_snapshot_previous: false,
            has_snapshot_original: false,
        };
        adapter.create_task(&task).map_err(to_fdo)
    }


    fn delete_task(&self, backend: &str, name: &str) -> fdo::Result<()> {
        info!(backend, name, "DeleteTask");
        self.adapter(backend)?.delete_task(name).map_err(to_fdo)
    }

    fn toggle_task(&self, backend: &str, name: &str, enable: bool) -> fdo::Result<()> {
        info!(backend, name, enable, "ToggleTask");
        self.adapter(backend)?
            .toggle_task(name, enable)
            .map_err(to_fdo)
    }

    fn revert_task(&self, backend: &str, name: &str, target: &str) -> fdo::Result<()> {
        info!(backend, name, target, "RevertTask");
        let tgt = SnapshotTarget::from_str(target).ok_or_else(|| {
            fdo::Error::InvalidArgs(format!("revert target: {target}"))
        })?;
        self.adapter(backend)?.revert_task(name, tgt).map_err(to_fdo)
    }

    /// Returns JSON-encoded `Vec<Task>` aggregated across every backend the
    /// daemon owns. Per-backend errors are logged and skipped so a broken
    /// backend doesn't poison the whole list.
    fn list_tasks(&self) -> fdo::Result<String> {
        let mut all = Vec::new();
        for (backend, adapter) in &self.adapters {
            match adapter.list_tasks() {
                Ok(tasks) => all.extend(tasks),
                Err(e) => error!(?backend, ?e, "list_tasks failed"),
            }
        }
        serde_json::to_string(&all).map_err(|e| {
            fdo::Error::Failed(format!("serialize: {e}"))
        })
    }

    // ── User-scope device tasks ───────────────────────────────────────────────
    // These write a bridge system service (User=username) + a udev rule so the
    // command runs as the caller's user when the device appears. Compatible with
    // any systemd >= 183; no SYSTEMD_USER_WANTS(uid) or RUN+= needed.

    fn create_user_device_task(
        &self,
        name: &str,
        command: &str,
        trigger_value: &str,
        uid: u32,
    ) -> fdo::Result<()> {
        info!(name, uid, "CreateUserDeviceTask");
        let (username, home) = lookup_uid(uid)
            .ok_or_else(|| fdo::Error::Failed(format!("unknown uid {uid}")))?;
        let m: DeviceMatch = serde_json::from_str(trigger_value)
            .map_err(|e| fdo::Error::InvalidArgs(format!("trigger: {e}")))?;
        let task = Task {
            name: name.to_string(),
            command: command.to_string(),
            trigger: Trigger::OnDevice(m.clone()),
            enabled: true,
            next_run: None,
            scope: Scope::User,
            backend: Backend::SystemdUser,
            origin: TaskOrigin::Owned,
            lifecycle: Lifecycle::Persistent,
            has_snapshot_previous: false,
            has_snapshot_original: false,
        };
        task_scheduler_core::validate_name(name).map_err(to_fdo)?;
        task_scheduler_core::ensure_not_protected(name).map_err(to_fdo)?;
        let snap_root = home.join(".local").join("share").join("task-scheduler").join("snapshots");
        self.systemd_root
            .create_user_device_task(&task, &m, &username, uid, &snap_root)
            .map_err(to_fdo)
    }

    fn delete_user_device_task(&self, name: &str, uid: u32) -> fdo::Result<()> {
        info!(name, uid, "DeleteUserDeviceTask");
        task_scheduler_core::validate_name(name).map_err(to_fdo)?;
        task_scheduler_core::ensure_not_protected(name).map_err(to_fdo)?;
        let (_, home) = lookup_uid(uid)
            .ok_or_else(|| fdo::Error::Failed(format!("unknown uid {uid}")))?;
        let snap_root = home.join(".local").join("share").join("task-scheduler").join("snapshots");
        self.systemd_root.delete_user_device_task(name, &snap_root).map_err(to_fdo)
    }

    fn toggle_user_device_task(&self, name: &str, enable: bool, uid: u32) -> fdo::Result<()> {
        info!(name, enable, uid, "ToggleUserDeviceTask");
        task_scheduler_core::validate_name(name).map_err(to_fdo)?;
        task_scheduler_core::ensure_not_protected(name).map_err(to_fdo)?;
        self.systemd_root.toggle_user_device_task(name, enable).map_err(to_fdo)
    }

    fn revert_user_device_task(&self, name: &str, target: &str, uid: u32) -> fdo::Result<()> {
        info!(name, target, uid, "RevertUserDeviceTask");
        task_scheduler_core::validate_name(name).map_err(to_fdo)?;
        task_scheduler_core::ensure_not_protected(name).map_err(to_fdo)?;
        let tgt = SnapshotTarget::from_str(target)
            .ok_or_else(|| fdo::Error::InvalidArgs(format!("revert target: {target}")))?;
        let (_, home) = lookup_uid(uid)
            .ok_or_else(|| fdo::Error::Failed(format!("unknown uid {uid}")))?;
        let snap_root = home.join(".local").join("share").join("task-scheduler").join("snapshots");
        self.systemd_root
            .revert_user_device_task(name, tgt, &snap_root)
            .map_err(to_fdo)
    }
}

/// Look up username and home directory for a UID using getpwuid_r (thread-safe).
fn lookup_uid(uid: u32) -> Option<(String, PathBuf)> {
    let mut buf: Vec<u8> = vec![0u8; 4096];
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
    let ret = unsafe {
        libc::getpwuid_r(
            uid,
            &mut pw,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        )
    };
    if ret != 0 || result.is_null() {
        return None;
    }
    let name = unsafe { CStr::from_ptr(pw.pw_name) }
        .to_string_lossy()
        .to_string();
    let home = unsafe { CStr::from_ptr(pw.pw_dir) }
        .to_string_lossy()
        .to_string();
    if name.is_empty() || home.is_empty() {
        return None;
    }
    Some((name, PathBuf::from(home)))
}

fn to_fdo(e: SchedulerError) -> fdo::Error {
    match e {
        SchedulerError::InvalidName(n) => {
            fdo::Error::InvalidArgs(format!("invalid task name: {n}"))
        }
        SchedulerError::NotRoot => {
            fdo::Error::AccessDenied("daemon not running as root".into())
        }
        SchedulerError::AuthDenied => {
            fdo::Error::AccessDenied("authentication required".into())
        }
        SchedulerError::UnsupportedBackend(s) => fdo::Error::NotSupported(s),
        SchedulerError::UnsupportedTrigger(s) => fdo::Error::NotSupported(s),
        SchedulerError::NoSnapshot => fdo::Error::Failed("no snapshot available".into()),
        SchedulerError::Protected(n) => fdo::Error::AccessDenied(format!(
            "'{n}' is managed by Task Scheduler itself and cannot be modified from the app"
        )),
        other => fdo::Error::Failed(other.to_string()),
    }
}


