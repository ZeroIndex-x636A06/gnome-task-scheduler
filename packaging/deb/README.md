# Debian / Ubuntu package

Build the `.deb` from the repository root:

```sh
# Install build deps once
sudo apt install build-essential debhelper devscripts cargo rustc \
  pkg-config libgtk-4-dev libadwaita-1-dev

# Stage the debian/ tree at the repo root (dpkg-buildpackage expects it there)
cp -r desktop/task-scheduler/packaging/deb/debian .

# Build
dpkg-buildpackage -b -uc -us

# Install the produced .deb (one level up)
sudo apt install ../task-scheduler_0.2.0-1_*.deb
```

The package:

- installs `/usr/bin/task-scheduler` and `/usr/bin/task-scheduler-daemon`
- installs the DBus system policy, polkit policy, systemd unit,
  `.desktop` entry, and icon
- enables and starts `task-scheduler-daemon.service` on first install
- stops and disables the service on removal

Min Rust version: 1.75. On older Debian / Ubuntu releases install
`rustup` and use a current toolchain instead of the distro `rustc`.
