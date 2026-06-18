# Task Scheduler (Linux / GNOME)

A native, Windows-Task-Scheduler-style app for Linux. Rust + GTK4 + libadwaita
on the frontend, a small zbus DBus daemon on the backend for system-wide
tasks.

## Workspace

```text
core/    Shared Task / Trigger / TaskScheduler trait
gui/     GTK4 + libadwaita application (unprivileged)
daemon/  zbus system-bus daemon, runs as root
packaging/  DBus policy + systemd unit
install.sh  Build + deploy
```

## Build & run (development)

System packages (Debian/Ubuntu):

```bash
sudo apt install build-essential pkg-config \
                 libgtk-4-dev libadwaita-1-dev libdbus-1-dev
```

User-scope tasks only (no daemon):

```bash
cargo run -p task-scheduler
```

## Install the privileged daemon

```bash
sudo ./install.sh
```

This builds both binaries, drops the daemon in `/usr/local/bin/`, installs
`/etc/dbus-1/system.d/org.linux.TaskScheduler.conf`, and enables
`task-scheduler-daemon.service`. Toggle **"Run as system (root)"** in the
New Task dialog to send the request over DBus instead of writing to your
user systemd dir.

## Security notes (Phase 2)

The DBus policy currently allows **any local user** to invoke the daemon.
Polkit per-method authorization is planned for Phase 3.
