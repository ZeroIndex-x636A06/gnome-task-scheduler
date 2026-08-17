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
REAL_HOME="$(eval echo "~${REAL_USER}")"

# ── Dependency installer ──────────────────────────────────────────────────────
detect_pm() {
  if   command -v apt-get &>/dev/null; then echo apt
  elif command -v dnf     &>/dev/null; then echo dnf
  elif command -v pacman  &>/dev/null; then echo pacman
  else echo unknown
  fi
}

install_build_deps() {
  local pm; pm=$(detect_pm)
  echo "==> Detected package manager: ${pm}"

  # Avoid installing a system rust package if rustup already provides cargo.
  local need_rust=true
  if command -v cargo &>/dev/null \
      || [[ -x "${REAL_HOME}/.cargo/bin/cargo" ]]; then
    need_rust=false
    echo "    cargo already available — skipping rust installation"
  fi

  case "$pm" in
    apt)
      apt-get update -qq
      local pkgs=(pkg-config libgtk-4-dev libadwaita-1-dev libglib2.0-dev
                  dbus systemd)
      # policykit-1 was split up and dropped in Debian 13 / Ubuntu 24.04.
      # Pick whichever name this release actually ships rather than pinning one.
      if apt-cache show polkitd >/dev/null 2>&1; then
        pkgs+=(polkitd)
      else
        pkgs+=(policykit-1)
      fi
      $need_rust && pkgs+=(cargo rustc)
      apt-get install -y "${pkgs[@]}"
      ;;
    dnf)
      local pkgs=(pkgconf-pkg-config gtk4-devel libadwaita-devel glib2-devel
                  dbus polkit systemd)
      $need_rust && pkgs+=(rust cargo)
      dnf install -y "${pkgs[@]}"
      ;;
    pacman)
      local pkgs=(pkgconf gtk4 libadwaita dbus polkit systemd)
      $need_rust && pkgs+=(rust)
      # Only touch pacman if something is genuinely missing. A bare `-Sy`
      # leaves the system in a partial-upgrade state, which is itself the
      # usual cause of mismatched/"outdated" library errors later on.
      local missing=()
      local p
      for p in "${pkgs[@]}"; do
        pacman -Qq "$p" &>/dev/null || missing+=("$p")
      done
      if (( ${#missing[@]} )); then
        echo "    installing: ${missing[*]}"
        pacman -Syu --needed --noconfirm "${missing[@]}"
      else
        echo "    all dependencies already present — nothing to do"
      fi
      ;;
    *)
      echo "  warn: Unrecognised package manager. Install manually:" >&2
      echo "    - rust + cargo  (https://rustup.rs)" >&2
      echo "    - pkg-config / pkgconf" >&2
      echo "    - GTK4 dev headers  (libgtk-4-dev / gtk4-devel / gtk4)" >&2
      echo "    - libadwaita dev headers  (libadwaita-1-dev / libadwaita-devel / libadwaita)" >&2
      ;;
  esac
}

install_build_deps

# ── Build ─────────────────────────────────────────────────────────────────────
# Augment PATH so rustup-managed cargo is found even when sudo strips $HOME.
AUGMENTED_PATH="${REAL_HOME}/.cargo/bin:${PATH}"

echo "==> Building workspace (release) as ${REAL_USER}"
sudo -u "$REAL_USER" env PATH="$AUGMENTED_PATH" \
  cargo build --release --workspace

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
install -Dm644 packaging/org.linux.TaskScheduler.desktop \
  /usr/share/applications/org.linux.TaskScheduler.desktop
install -Dm644 packaging/task-scheduler.png \
  /usr/share/icons/hicolor/512x512/apps/task-scheduler.png
update-desktop-database /usr/share/applications 2>/dev/null || true
gtk-update-icon-cache -f /usr/share/icons/hicolor 2>/dev/null || true

echo "==> Reloading dbus + systemd"
# Reload only — never `restart dbus` on a live system. Restarting the system
# bus tears down every client attached to it and can log the user out mid-install.
systemctl reload dbus 2>/dev/null || true
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
