# Fedora / RHEL / openSUSE package

Build the `.rpm` from a release tarball:

```sh
# Install build deps once
sudo dnf install rpm-build rust cargo pkgconf-pkg-config \
  gtk4-devel libadwaita-devel systemd-rpm-macros

# Set up an rpmbuild tree
rpmdev-setuptree   # from package `rpmdevtools`

# Drop the spec and a v0.2.0 source tarball into the tree
cp desktop/task-scheduler/packaging/rpm/task-scheduler.spec \
   ~/rpmbuild/SPECS/
spectool -g -R ~/rpmbuild/SPECS/task-scheduler.spec
# (or wget the GitHub tag tarball into ~/rpmbuild/SOURCES/)

# Build
rpmbuild -ba ~/rpmbuild/SPECS/task-scheduler.spec

# Install
sudo dnf install ~/rpmbuild/RPMS/*/task-scheduler-0.2.0-1.*.rpm
```

The package:

- installs `/usr/bin/task-scheduler` and `/usr/bin/task-scheduler-daemon`
- installs the DBus system policy, polkit policy, systemd unit,
  `.desktop` entry, and icon
- enables and starts `task-scheduler-daemon.service` on first install
- stops and disables the service on removal

openSUSE: the same spec works with `osc build` after copying it into
your home project; deps are the same package names.

Min Rust version: 1.75.
