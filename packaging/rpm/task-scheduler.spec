Name:           task-scheduler
Version:        0.2.0
Release:        1%{?dist}
Summary:        GTK4/libadwaita scheduler for system and user tasks

License:        MIT
URL:            https://github.com/ZeroIndex-x636A06/gnome-task-manager
Source0:        %{url}/archive/refs/tags/v%{version}.tar.gz#/%{name}-%{version}.tar.gz

BuildRequires:  rust >= 1.75
BuildRequires:  cargo
BuildRequires:  pkgconf-pkg-config
BuildRequires:  gtk4-devel
BuildRequires:  libadwaita-devel
BuildRequires:  systemd-rpm-macros

Requires:       gtk4
Requires:       libadwaita
Requires:       dbus
Requires:       polkit
Requires:       systemd
Recommends:     cronie
Recommends:     breeze-icon-theme
Recommends:     xfconf

%{?systemd_requires}

%description
Task Scheduler is a desktop frontend for scheduling cron and systemd
timer tasks. It ships a privileged DBus daemon that performs the
system-scope work, gated behind polkit, and a GTK4/libadwaita GUI.

%prep
%autosetup -n %{name}-%{version}

%build
cargo build --release --workspace --locked

%install
install -Dm755 target/release/task-scheduler        %{buildroot}%{_bindir}/task-scheduler
install -Dm755 target/release/task-scheduler-daemon %{buildroot}%{_bindir}/task-scheduler-daemon

install -Dm644 packaging/org.linux.TaskScheduler.conf \
  %{buildroot}%{_datadir}/dbus-1/system.d/org.linux.TaskScheduler.conf

install -Dm644 packaging/org.linux.TaskScheduler.policy \
  %{buildroot}%{_datadir}/polkit-1/actions/org.linux.TaskScheduler.policy

install -d %{buildroot}%{_unitdir}
sed 's|/usr/local/bin/|%{_bindir}/|g' packaging/task-scheduler-daemon.service \
  > %{buildroot}%{_unitdir}/task-scheduler-daemon.service
chmod 644 %{buildroot}%{_unitdir}/task-scheduler-daemon.service

install -Dm644 packaging/task-scheduler.desktop \
  %{buildroot}%{_datadir}/applications/task-scheduler.desktop
install -Dm644 packaging/task-scheduler.png \
  %{buildroot}%{_datadir}/icons/hicolor/512x512/apps/task-scheduler.png

install -d -m 0755 %{buildroot}%{_sharedstatedir}/task-scheduler/snapshots

%post
%systemd_post task-scheduler-daemon.service
systemctl reload dbus 2>/dev/null || systemctl restart dbus || true
systemctl enable --now task-scheduler-daemon.service || true
update-desktop-database %{_datadir}/applications 2>/dev/null || true
gtk-update-icon-cache -f %{_datadir}/icons/hicolor 2>/dev/null || true

%preun
%systemd_preun task-scheduler-daemon.service

%postun
%systemd_postun_with_restart task-scheduler-daemon.service
systemctl reload dbus 2>/dev/null || true
update-desktop-database %{_datadir}/applications 2>/dev/null || true
gtk-update-icon-cache -f %{_datadir}/icons/hicolor 2>/dev/null || true

%files
%license LICENSE
%{_bindir}/task-scheduler
%{_bindir}/task-scheduler-daemon
%{_datadir}/dbus-1/system.d/org.linux.TaskScheduler.conf
%{_datadir}/polkit-1/actions/org.linux.TaskScheduler.policy
%{_unitdir}/task-scheduler-daemon.service
%{_datadir}/applications/task-scheduler.desktop
%{_datadir}/icons/hicolor/512x512/apps/task-scheduler.png
%dir %{_sharedstatedir}/task-scheduler
%dir %{_sharedstatedir}/task-scheduler/snapshots

%changelog
* Wed Jun 18 2026 Caleb Jarrell <calebjarrell2006@gmail.com> - 0.2.0-1
- Initial RPM packaging.
