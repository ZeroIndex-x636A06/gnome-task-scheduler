# Task Scheduler

A native GUI for scheduling recurring tasks on Linux — the closest thing to Windows Task Scheduler on the desktop. Built with Rust, GTK4, and libadwaita.

Linux has powerful scheduling tools (`cron`, systemd timers), but they all require editing config files by hand. Task Scheduler gives you a polished interface that writes proper systemd `.service` and `.timer` units under the hood, so your tasks survive reboots and show up in `journalctl` like any other service.

![Task Scheduler screenshot](https://github.com/user-attachments/assets/placeholder)

## Features

- Schedule tasks by time, interval, or device attach (udev)
- User tasks run without root; system-wide tasks go through a privileged DBus daemon gated by polkit
- Feels native on GNOME, KDE, XFCE, and tiling WMs (Sway, Hyprland, i3)
- Tasks are plain systemd units — no lock-in, manageable with standard tools

## Installation

### From a package (recommended)

Download the `.deb` or `.rpm` for your distro from the [latest release](https://github.com/ZeroIndex-x636A06/gnome-task-scheduler/releases/latest).

```bash
# Debian / Ubuntu / Mint
sudo dpkg -i task-scheduler_*.deb

# Fedora / RHEL
sudo rpm -i task-scheduler-*.rpm
```

### From source

**Prerequisites:** Rust (stable), `pkg-config`, `gtk4`, `libadwaita`, `dbus`, `polkit`

<details>
<summary>Install build dependencies (optional — <code>install.sh</code> does this for you)</summary>

`install.sh` detects your package manager and installs these itself, so you
normally don't need to run anything here. Listed for reference, or if you'd
rather install them yourself first.

**Debian / Ubuntu / Mint**
```bash
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev polkitd
```
> On releases older than Debian 13 / Ubuntu 24.04, `polkitd` is called `policykit-1`.

**Arch / Manjaro / EndeavourOS / CachyOS**
```bash
sudo pacman -S --needed pkgconf gtk4 libadwaita dbus polkit systemd
```
> Rust comes from `rust`, or from [rustup](https://rustup.rs) if you already use it — the installer detects either.

**Fedora / RHEL**
```bash
sudo dnf install pkgconf-pkg-config gtk4-devel libadwaita-devel polkit
```
</details>

Clone and run the install script:

```bash
git clone https://github.com/ZeroIndex-x636A06/gnome-task-scheduler.git task-scheduler
cd task-scheduler
sudo ./install.sh
```

The script builds both binaries, installs them to `/usr/local/bin/`, sets up the DBus policy, registers the systemd daemon, and installs the desktop entry and icon.

## Usage

Launch from your application menu, or run:

```bash
task-scheduler
```

**Creating a task:** Click **New Task**, fill in the name, command, and trigger, then click **Save**.

- **User tasks** (default) are stored in `~/.config/systemd/user/` — no root required.
- **System tasks** toggle **"Run as system (root)"** in the dialog. The privileged daemon writes the unit to `/etc/systemd/system/`.

**Viewing logs for a task:**
```bash
journalctl -u <task-name>.service -f
```

## Uninstalling

From the cloned repo directory:

```bash
sudo ./uninstall.sh
```

To also delete every system-wide task created by Task Scheduler:

```bash
sudo ./uninstall.sh --purge-tasks
```

This removes all installed binaries, the DBus policy, polkit policy, systemd unit, desktop entry, icon, and (with `--purge-tasks`) all task units and udev rules written by the daemon. User tasks in `~/.config/systemd/user/` are never touched.

## Desktop environment support

| Environment | Notes |
|---|---|
| GNOME | Native Adwaita look and feel |
| KDE Plasma | Reads accent color from `~/.config/kdeglobals` |
| XFCE / Cinnamon / MATE | Detects dark mode |
| Sway / Hyprland / i3 | Hides title-bar controls |

## Development

Run the GUI without installing the daemon:

```bash
cargo run -p task-scheduler
```

User-scope tasks work fully in this mode. System tasks require the daemon to be running.

**Workspace layout:**
```
core/        Shared types (Task, Trigger, TaskScheduler trait)
gui/         GTK4 + libadwaita frontend
daemon/      Privileged zbus system-bus daemon
packaging/   DBus policy, polkit policy, systemd unit, desktop entry
```

## Security

The DBus policy currently allows any local user to invoke the daemon. Polkit per-method authorization is planned — once added, scheduling system-wide tasks will prompt for your administrator password.
