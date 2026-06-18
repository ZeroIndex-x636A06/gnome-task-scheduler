//! Adapter registry — every `TaskScheduler` the GUI can route to plus the
//! host profile that drove the decisions.

pub mod autostart;
pub mod cron_user;
pub mod detect;
pub mod systemd_root_proxy;
pub mod systemd_user;

use std::rc::Rc;

pub use autostart::AutostartAdapter;
pub use cron_user::CronUserAdapter;
pub use detect::{probe, HostProfile, InitSystem};
pub use systemd_root_proxy::DaemonProxy;
pub use systemd_user::SystemdUserAdapter;
pub use task_scheduler_core::{
    Backend, SchedulerError, SnapshotTarget, Task, TaskOrigin,
    TaskScheduler, Trigger,
};


#[derive(Clone)]
pub struct Registry {
    pub profile: HostProfile,
    adapters: Vec<Rc<dyn TaskScheduler>>,
}

impl Registry {
    pub fn build() -> Self {
        let profile = probe();
        let mut adapters: Vec<Rc<dyn TaskScheduler>> = Vec::new();

        // User-scope adapters are always cheap to instantiate. We expose them
        // when the underlying tooling exists.
        if profile.init == InitSystem::Systemd {
            adapters.push(Rc::new(SystemdUserAdapter::new(profile.daemon_present)));
        }
        if profile.has_cron {
            adapters.push(Rc::new(CronUserAdapter::new()));
        }
        if profile.has_autostart_dir {
            adapters.push(Rc::new(AutostartAdapter::new()));
        }

        // System-scope adapters only when the daemon answers. The daemon
        // itself decides which backends it actually supports — we list the
        // three the daemon owns so the UI can offer them.
        if profile.daemon_present {
            if profile.init == InitSystem::Systemd {
                adapters.push(Rc::new(DaemonProxy::new(Backend::SystemdSystem)));
            }
            if profile.has_cron {
                adapters.push(Rc::new(DaemonProxy::new(Backend::CronSystem)));
            }
            if profile.init == InitSystem::OpenRc {
                adapters.push(Rc::new(DaemonProxy::new(Backend::OpenRc)));
            }
        }

        Self { profile, adapters }
    }

    pub fn available(&self) -> &[Rc<dyn TaskScheduler>] { &self.adapters }

    pub fn by_backend(&self, backend: Backend) -> Option<Rc<dyn TaskScheduler>> {
        self.adapters.iter().find(|a| a.backend() == backend).cloned()
    }

    /// Aggregates tasks from every adapter; per-adapter failures are
    /// returned alongside successful tasks so the UI can toast them.
    pub fn list_all_tasks(&self) -> (Vec<Task>, Vec<(Backend, SchedulerError)>) {
        let mut tasks = Vec::new();
        let mut errors = Vec::new();
        for a in &self.adapters {
            match a.list_tasks() {
                Ok(mut t) => tasks.append(&mut t),
                Err(e) => errors.push((a.backend(), e)),
            }
        }
        tasks.sort_by(|a, b| {
            (a.backend.label(), &a.name).cmp(&(b.backend.label(), &b.name))
        });
        (tasks, errors)
    }
}
