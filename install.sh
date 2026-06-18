#!/usr/bin/env bash
# Build the Task Scheduler workspace and install the privileged daemon,
# its DBus policy, and the systemd unit that keeps it alive.
#
# Run from the repository root as: sudo ./install.sh
set -euo pipefail

if [[ $EUID -ne 0 ]]; then
  echo "install.sh must be run as root (use sudo)" >&2
  exit 1
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT_DIR"

# Resolve the invoking user so cargo's target dir stays in their $HOME and
# not in root-owned space.
REAL_USER="${SUDO_USER:-$USER}"
echo "==> Building workspace (release) as ${REAL_USER}"
sudo -u "$REAL_USER" cargo build --release --workspace

DAEMON_BIN="$ROOT_DIR/target/release/task-scheduler-daemon"
GUI_BIN="$ROOT_DIR/target/release/task-scheduler"

if [[ ! -x "$DAEMON_BIN" ]]; then
  echo "Missing built daemon at $DAEMON_BIN" >&2
  exit 1
fi

echo "==> Stopping daemon (if running) before replacing binary"
systemctl stop task-scheduler-daemon.service 2>/dev/null || true

echo "==> Installing daemon to /usr/local/bin/"
install -Dm755 "$DAEMON_BIN" /usr/local/bin/task-scheduler-daemon
if [[ -x "$GUI_BIN" ]]; then
  install -Dm755 "$GUI_BIN" /usr/local/bin/task-scheduler
fi

echo "==> Installing DBus system policy"
install -Dm644 packaging/org.linux.TaskScheduler.conf \
  /etc/dbus-1/system.d/org.linux.TaskScheduler.conf

echo "==> Installing polkit policy"
if [[ -d /usr/share/polkit-1/actions ]]; then
  install -Dm644 packaging/org.linux.TaskScheduler.policy \
    /usr/share/polkit-1/actions/org.linux.TaskScheduler.policy
else
  echo "    note: /usr/share/polkit-1/actions missing — polkit prompts unavailable."
fi

echo "==> Installing systemd unit"
install -Dm644 packaging/task-scheduler-daemon.service \
  /etc/systemd/system/task-scheduler-daemon.service

echo "==> Preparing snapshot directory"
install -d -m 0755 /var/lib/task-scheduler/snapshots

echo "==> Installing desktop entry and icons"
install -Dm644 packaging/task-scheduler.desktop \
  /usr/share/applications/task-scheduler.desktop
install -Dm644 packaging/task-scheduler.png \
  /usr/share/icons/hicolor/512x512/apps/task-scheduler.png
update-desktop-database /usr/share/applications 2>/dev/null || true
gtk-update-icon-cache -f /usr/share/icons/hicolor 2>/dev/null || true

echo "==> Reloading dbus + systemd"
systemctl reload dbus 2>/dev/null || systemctl restart dbus || true
systemctl daemon-reload
systemctl enable --now task-scheduler-daemon.service


echo "==> Verifying udev hooks (needed for hardware-attach triggers)"
if [[ ! -d /etc/udev/rules.d ]]; then
  echo "    note: /etc/udev/rules.d is missing — device triggers will be unavailable."
elif ! command -v udevadm >/dev/null 2>&1; then
  echo "    note: udevadm not in PATH — device triggers will be unavailable."
else
  echo "    ok"
fi

echo
echo "Done. Tail logs with:"
echo "    journalctl -u task-scheduler-daemon -f"
echo
echo "Launch the GUI with:  task-scheduler   (or cargo run -p task-scheduler)"
echo
echo "Hardware-attach triggers create files in /etc/udev/rules.d/ and"
echo "/etc/systemd/system/. They are removed when you delete the task."
