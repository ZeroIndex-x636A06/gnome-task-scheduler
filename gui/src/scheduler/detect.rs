//! Host probe. Determines which init system is running and which optional
//! tools are available, so the GUI can show only what the host supports.

use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitSystem {
    Systemd,
    OpenRc,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct HostProfile {
    pub init: InitSystem,
    pub has_cron: bool,
    pub has_autostart_dir: bool,
    pub daemon_present: bool,
}

pub fn probe() -> HostProfile {
    HostProfile {
        init: detect_init(),
        has_cron: which("crontab"),
        has_autostart_dir: dirs::config_dir()
            .map(|d| d.join("autostart"))
            .map(|p| p.exists() || std::fs::create_dir_all(&p).is_ok())
            .unwrap_or(false),
        daemon_present: daemon_present(),
    }
}

fn detect_init() -> InitSystem {
    let comm = std::fs::read_to_string("/proc/1/comm")
        .unwrap_or_default()
        .trim()
        .to_string();
    match comm.as_str() {
        "systemd" => InitSystem::Systemd,
        "openrc-init" => InitSystem::OpenRc,
        // Some OpenRC setups keep BusyBox/SysV-style `init` as pid 1.
        "init" if Path::new("/run/openrc").exists() => InitSystem::OpenRc,
        _ if Path::new("/run/openrc").exists() => InitSystem::OpenRc,
        _ if Path::new("/run/systemd/system").exists() => InitSystem::Systemd,
        _ => InitSystem::Unknown,
    }
}

fn which(bin: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {bin}")])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn daemon_present() -> bool {
    // Best-effort: query the system bus for the well-known name. Errors mean
    // "no daemon" — never panic, never block longer than a few ms.
    let Ok(conn) = zbus::blocking::Connection::system() else { return false; };
    let Ok(proxy) = zbus::blocking::fdo::DBusProxy::new(&conn) else { return false; };
    let name: zbus::names::BusName<'_> = match "org.linux.TaskScheduler".try_into() {
        Ok(n) => n,
        Err(_) => return false,
    };
    proxy.name_has_owner(name).unwrap_or(false)
}
