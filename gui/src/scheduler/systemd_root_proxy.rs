//! DBus proxy for any backend hosted by the privileged daemon. A `kind`
//! constructor picks the `Backend` tag this proxy presents to the GUI.

use task_scheduler_core::{
    Backend, Capabilities, SchedulerError, SnapshotTarget, Task, TaskScheduler,
};


use zbus::blocking::Connection;

const BUS_NAME: &str = "org.linux.TaskScheduler";
const OBJECT_PATH: &str = "/org/linux/TaskScheduler";
const INTERFACE: &str = "org.linux.TaskScheduler";

#[zbus::proxy(
    interface = "org.linux.TaskScheduler",
    default_service = "org.linux.TaskScheduler",
    default_path = "/org/linux/TaskScheduler",
    gen_blocking = true
)]
trait Scheduler {
    fn create_task(
        &self,
        backend: &str,
        name: &str,
        command: &str,
        trigger_kind: &str,
        trigger_value: &str,
        lifecycle: &str,
    ) -> zbus::Result<()>;

    fn delete_task(&self, backend: &str, name: &str) -> zbus::Result<()>;
    fn list_tasks(&self) -> zbus::Result<String>;
    fn toggle_task(&self, backend: &str, name: &str, enable: bool) -> zbus::Result<()>;
    fn revert_task(&self, backend: &str, name: &str, target: &str) -> zbus::Result<()>;

    // User-scope device task operations (handled by root daemon, run as uid).
    fn create_user_device_task(
        &self, name: &str, command: &str, trigger_value: &str, uid: u32,
    ) -> zbus::Result<()>;
    fn delete_user_device_task(&self, name: &str, uid: u32) -> zbus::Result<()>;
    fn toggle_user_device_task(&self, name: &str, enable: bool, uid: u32) -> zbus::Result<()>;
    fn revert_user_device_task(&self, name: &str, target: &str, uid: u32) -> zbus::Result<()>;
}

/// Thin helper used by `SystemdUserAdapter` to call the daemon for the
/// root-requiring parts of user-scope device tasks (udev rule + bridge service).
pub struct UserDeviceBridge {
    uid: u32,
}

impl UserDeviceBridge {
    pub fn new(uid: u32) -> Self { Self { uid } }

    fn proxy() -> Result<SchedulerProxyBlocking<'static>, task_scheduler_core::SchedulerError> {
        let conn = Connection::system().map_err(map_err)?;
        SchedulerProxyBlocking::builder(&conn)
            .destination(BUS_NAME).map_err(map_err)?
            .path(OBJECT_PATH).map_err(map_err)?
            .interface(INTERFACE).map_err(map_err)?
            .build().map_err(map_err)
    }

    pub fn create(
        &self, name: &str, command: &str, trigger_value: &str,
    ) -> Result<(), task_scheduler_core::SchedulerError> {
        Self::proxy()?
            .create_user_device_task(name, command, trigger_value, self.uid)
            .map_err(map_err)
    }

    pub fn delete(&self, name: &str) -> Result<(), task_scheduler_core::SchedulerError> {
        Self::proxy()?.delete_user_device_task(name, self.uid).map_err(map_err)
    }

    pub fn toggle(&self, name: &str, enable: bool) -> Result<(), task_scheduler_core::SchedulerError> {
        Self::proxy()?.toggle_user_device_task(name, enable, self.uid).map_err(map_err)
    }

    pub fn revert(
        &self, name: &str, target: task_scheduler_core::SnapshotTarget,
    ) -> Result<(), task_scheduler_core::SchedulerError> {
        Self::proxy()?
            .revert_user_device_task(name, target.as_str(), self.uid)
            .map_err(map_err)
    }
}


/// Adapter that proxies one specific `Backend` over DBus. `list_tasks`
/// filters the daemon's aggregated response to this backend only.
pub struct DaemonProxy {
    backend: Backend,
}

impl DaemonProxy {
    pub fn new(backend: Backend) -> Self { Self { backend } }

    fn proxy(&self) -> Result<SchedulerProxyBlocking<'static>, SchedulerError> {
        let conn = Connection::system().map_err(map_err)?;
        SchedulerProxyBlocking::builder(&conn)
            .destination(BUS_NAME).map_err(map_err)?
            .path(OBJECT_PATH).map_err(map_err)?
            .interface(INTERFACE).map_err(map_err)?
            .build().map_err(map_err)
    }
}

impl TaskScheduler for DaemonProxy {
    fn backend(&self) -> Backend { self.backend }

    fn capabilities(&self) -> Capabilities {
        match self.backend {
            Backend::SystemdSystem => Capabilities {
                on_calendar: true, on_boot: true, on_login: false, on_device: true,
                enable_toggle: true, system_scope: true,
            },
            Backend::CronSystem => Capabilities {
                on_calendar: true, on_boot: false, on_login: false, on_device: false,
                enable_toggle: true, system_scope: true,
            },
            Backend::OpenRc => Capabilities {
                on_calendar: false, on_boot: true, on_login: false, on_device: false,
                enable_toggle: true, system_scope: true,
            },
            _ => Capabilities::none(),
        }
    }

    fn create_task(&self, task: &Task) -> Result<(), SchedulerError> {
        self.proxy()?
            .create_task(
                self.backend.tag(),
                &task.name,
                &task.command,
                task.trigger.kind(),
                &task.trigger.value(),
                task.lifecycle.as_str(),
            )
            .map_err(map_err)
    }


    fn delete_task(&self, name: &str) -> Result<(), SchedulerError> {
        self.proxy()?.delete_task(self.backend.tag(), name).map_err(map_err)
    }

    fn toggle_task(&self, name: &str, enable: bool) -> Result<(), SchedulerError> {
        self.proxy()?
            .toggle_task(self.backend.tag(), name, enable)
            .map_err(map_err)
    }

    fn revert_task(&self, name: &str, target: SnapshotTarget) -> Result<(), SchedulerError> {
        self.proxy()?
            .revert_task(self.backend.tag(), name, target.as_str())
            .map_err(map_err)
    }

    fn list_tasks(&self) -> Result<Vec<Task>, SchedulerError> {
        let json = self.proxy()?.list_tasks().map_err(map_err)?;
        let mut all: Vec<Task> = serde_json::from_str(&json)
            .map_err(|e| SchedulerError::Parse(e.to_string()))?;
        all.retain(|t| t.backend == self.backend);
        Ok(all)
    }
}


fn map_err<E: std::fmt::Display>(e: E) -> SchedulerError {
    SchedulerError::Ipc(e.to_string())
}
