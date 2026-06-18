//! Privileged DBus daemon. Owns `org.linux.TaskScheduler` on the system bus
//! and dispatches CreateTask/etc. to the per-backend root adapters.

mod cron_root;
mod openrc_root;
mod service;
mod systemd_root;

use anyhow::{bail, Context, Result};
use tracing::info;
use zbus::connection;

const BUS_NAME: &str = "org.linux.TaskScheduler";
const OBJECT_PATH: &str = "/org/linux/TaskScheduler";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Safety: geteuid is a thread-safe FFI call returning uid_t.
    let euid = unsafe { libc::geteuid() };
    if euid != 0 {
        bail!("task-scheduler-daemon must run as root (euid={euid})");
    }

    let iface = service::SchedulerService::new();

    let _conn = connection::Builder::system()
        .context("connect to system bus")?
        .name(BUS_NAME)
        .context("request bus name")?
        .serve_at(OBJECT_PATH, iface)
        .context("register interface")?
        .build()
        .await
        .context("build dbus connection")?;

    info!("task-scheduler-daemon listening on {BUS_NAME} {OBJECT_PATH}");
    tokio::signal::ctrl_c().await.context("wait for shutdown signal")?;
    info!("shutting down");
    Ok(())
}
