# Task Scheduler packaging

Native Linux packages for Task Scheduler. Each one installs the same
set of files: the GUI binary, the privileged daemon, the system DBus
policy, the polkit policy, the systemd unit, a `.desktop` entry, and an
app icon.

| Distro family | Recipe | Build command |
|---|---|---|
| Arch / Manjaro / EndeavourOS | [`aur/task-scheduler/`](aur/task-scheduler/) (stable) or [`aur/task-scheduler-git/`](aur/task-scheduler-git/) | `makepkg -si` |
| Debian / Ubuntu / Mint | [`deb/`](deb/) | see [`deb/README.md`](deb/README.md) |
| Fedora / RHEL / openSUSE | [`rpm/`](rpm/) | see [`rpm/README.md`](rpm/README.md) |

All three install layouts match:

| File | Destination |
|---|---|
| `task-scheduler` | `/usr/bin/task-scheduler` |
| `task-scheduler-daemon` | `/usr/bin/task-scheduler-daemon` |
| `org.linux.TaskScheduler.conf` | `/usr/share/dbus-1/system.d/` |
| `org.linux.TaskScheduler.policy` | `/usr/share/polkit-1/actions/` |
| `task-scheduler-daemon.service` | `/usr/lib/systemd/system/` |
| `task-scheduler.desktop` | `/usr/share/applications/` |
| `task-scheduler.png` | `/usr/share/icons/hicolor/512x512/apps/` |
| `task-scheduler.svg` | `/usr/share/icons/hicolor/scalable/apps/` |

On install, the package enables and starts
`task-scheduler-daemon.service` and reloads the system DBus daemon so
the new bus policy takes effect. On removal the service is stopped and
disabled.

The systemd unit shipped here (`/usr/lib/...`) is identical to the one
in `install.sh`, except `ExecStart=` points at `/usr/bin/` instead of
`/usr/local/bin/`. Package scripts patch it during build.

For from-source installs without a package manager, use the
`install.sh` in the repository root.

## Desktop-environment polish

The GUI auto-detects KDE Plasma, XFCE, Cinnamon, MATE, and tiling WMs
(Sway / Hyprland / i3) at startup and adjusts header-bar chrome,
window decorations, and dark/accent colors accordingly. KDE accent is
read from `~/.config/kdeglobals`; XFCE dark mode is sniffed via
`xfconf-query`. None of this is required — on GNOME the app stays
pure Adwaita.

For best icon matching on KDE, install `breeze-icon-theme`. On
tiling WMs the title-bar controls are hidden so the compositor's
own borders take over.
