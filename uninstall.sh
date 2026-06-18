#!/usr/bin/env bash
# Remove everything install.sh placed on the system. Safe to run repeatedly.
# Does NOT touch user-scope tasks in ~/.config/systemd/user/ — those belong
# to the unprivileged GUI and you can manage them from the app.
#
# Usage: sudo ./uninstall.sh [--purge-tasks]
#   --purge-tasks   Also delete every /etc/systemd/system/*.{service,timer}
#                   created by Task Scheduler (matched by the unit's
#                   "Description=Task Scheduler:" line).
set -euo pipefail

if [[ $EUID -ne 0 ]]; then
  echo "uninstall.sh must be run as root (use sudo)" >&2
  exit 1
fi

PURGE_TASKS=0
for arg in "$@"; do
  case "$arg" in
    --purge-tasks) PURGE_TASKS=1 ;;
    -h|--help)
      sed -n '2,12p' "$0"; exit 0 ;;
    *) echo "unknown flag: $arg" >&2; exit 2 ;;
  esac
done

echo "==> Stopping and disabling daemon"
systemctl disable --now task-scheduler-daemon.service 2>/dev/null || true

echo "==> Removing installed files"
rm -f /etc/systemd/system/task-scheduler-daemon.service
rm -f /etc/dbus-1/system.d/org.linux.TaskScheduler.conf
rm -f /usr/share/polkit-1/actions/org.linux.TaskScheduler.policy
rm -f /usr/local/bin/task-scheduler-daemon
rm -f /usr/local/bin/task-scheduler
rm -f /usr/share/applications/task-scheduler.desktop
rm -f /usr/share/icons/hicolor/512x512/apps/task-scheduler.png
rm -f /usr/share/icons/hicolor/scalable/apps/task-scheduler.svg
update-desktop-database /usr/share/applications 2>/dev/null || true
gtk-update-icon-cache -f /usr/share/icons/hicolor 2>/dev/null || true


if [[ $PURGE_TASKS -eq 1 ]]; then
  echo "==> Purging system tasks created by Task Scheduler"
  shopt -s nullglob
  for unit in /etc/systemd/system/*.timer; do
    stem="${unit##*/}"; stem="${stem%.timer}"
    svc="/etc/systemd/system/${stem}.service"
    if [[ -f "$svc" ]] && grep -q "^Description=Task Scheduler:" "$svc"; then
      echo "    - ${stem}"
      systemctl disable --now "${stem}.timer" 2>/dev/null || true
      rm -f "$unit" "$svc"
    fi
  done
  # Hardware-attach (udev) tasks: paired rule + service files.
  for rule in /etc/udev/rules.d/90-task-scheduler-*.rules; do
    base="${rule##*/90-task-scheduler-}"; name="${base%.rules}"
    svc="/etc/systemd/system/task-scheduler-device-${name}.service"
    echo "    - device:${name}"
    rm -f "$rule" "$svc"
  done
  command -v udevadm >/dev/null 2>&1 && udevadm control --reload 2>/dev/null || true
  # Snapshot store (revert history).
  rm -rf /var/lib/task-scheduler
fi


echo "==> Reloading dbus + systemd"
systemctl daemon-reload
systemctl reload dbus 2>/dev/null || systemctl restart dbus || true

echo
echo "Done. To also remove cargo build artifacts:"
echo "    cargo clean --manifest-path \"$(cd \"$(dirname \"${BASH_SOURCE[0]}\")\" && pwd)/Cargo.toml\""
if [[ $PURGE_TASKS -eq 0 ]]; then
  echo
  echo "System tasks were left in place. Re-run with --purge-tasks to delete them."
fi
