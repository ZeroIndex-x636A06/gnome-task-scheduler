//! Shared types for the Task Scheduler GUI and root daemon.

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub mod snapshot;


#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum Backend {
    SystemdUser,
    SystemdSystem,
    CronUser,
    CronSystem,
    OpenRc,
    Autostart,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Backend::SystemdUser => "systemd (user)",
            Backend::SystemdSystem => "systemd (system)",
            Backend::CronUser => "cron (user)",
            Backend::CronSystem => "cron (system)",
            Backend::OpenRc => "OpenRC (system)",
            Backend::Autostart => "Autostart (login)",
        }
    }

    /// One-line explanation of what this backend does and where the task lives.
    pub fn description(self) -> &'static str {
        match self {
            Backend::SystemdUser =>
                "Runs as your user via `systemctl --user`. Unit files live in ~/.config/systemd/user/. Only fires while you are logged in; no root needed.",
            Backend::SystemdSystem =>
                "Runs system-wide as root via systemd. Unit files are written to /etc/systemd/system/ by the privileged daemon. Fires at boot, independent of any login session.",
            Backend::CronUser =>
                "Adds an entry to your user crontab (`crontab -e`). Runs as you whenever cron is active. Calendar/interval triggers only.",
            Backend::CronSystem =>
                "Adds an entry to /etc/crontab as root via the privileged daemon. Runs system-wide on a schedule.",
            Backend::OpenRc =>
                "Drops a script into /etc/local.d/ (via the daemon). Executes once at boot on OpenRC systems. No calendar or login triggers.",
            Backend::Autostart =>
                "Writes a freedesktop .desktop file to ~/.config/autostart/. Launches the command when you log into your desktop session.",
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            Backend::SystemdUser => "systemd-user",
            Backend::SystemdSystem => "systemd-system",
            Backend::CronUser => "cron-user",
            Backend::CronSystem => "cron-system",
            Backend::OpenRc => "openrc",
            Backend::Autostart => "autostart",
        }
    }

    pub fn from_tag(s: &str) -> Result<Self, SchedulerError> {
        match s {
            "systemd-user" => Ok(Backend::SystemdUser),
            "systemd-system" => Ok(Backend::SystemdSystem),
            "cron-user" => Ok(Backend::CronUser),
            "cron-system" => Ok(Backend::CronSystem),
            "openrc" => Ok(Backend::OpenRc),
            "autostart" => Ok(Backend::Autostart),
            other => Err(SchedulerError::Parse(format!("unknown backend: {other}"))),
        }
    }

    /// Backends fulfilled by the privileged daemon.
    pub fn requires_daemon(self) -> bool {
        matches!(
            self,
            Backend::SystemdSystem | Backend::CronSystem | Backend::OpenRc
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scope {
    User,
    System,
}

impl Scope {
    pub fn label(self) -> &'static str {
        match self {
            Scope::User => "User",
            Scope::System => "System",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Capabilities {
    pub on_calendar: bool,
    pub on_boot: bool,
    pub on_login: bool,
    pub on_device: bool,
    pub enable_toggle: bool,
    pub system_scope: bool,
}

impl Capabilities {
    pub const fn none() -> Self {
        Self {
            on_calendar: false,
            on_boot: false,
            on_login: false,
            on_device: false,
            enable_toggle: false,
            system_scope: false,
        }
    }

    pub fn supports(self, trigger: &Trigger) -> bool {
        match trigger {
            Trigger::OnCalendar(_) => self.on_calendar,
            Trigger::OnBootSec(_) => self.on_boot,
            Trigger::OnLogin => self.on_login,
            Trigger::OnDevice(_) => self.on_device,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum DeviceAction {
    Add,
    Remove,
    Change,
}

impl DeviceAction {
    pub fn as_str(self) -> &'static str {
        match self {
            DeviceAction::Add => "add",
            DeviceAction::Remove => "remove",
            DeviceAction::Change => "change",
        }
    }
    pub fn human(self) -> &'static str {
        match self {
            DeviceAction::Add => "connected",
            DeviceAction::Remove => "disconnected",
            DeviceAction::Change => "changed",
        }
    }
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "add" => Some(Self::Add),
            "remove" => Some(Self::Remove),
            "change" => Some(Self::Change),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceMatch {
    pub action: DeviceAction,
    pub subsystem: String,
    pub vendor_id: Option<String>,
    pub product_id: Option<String>,
    pub kernel: Option<String>,
    #[serde(default)]
    pub attrs: Vec<(String, String)>,
    /// Optional human label captured at picker time (e.g. "SanDisk Cruzer").
    #[serde(default)]
    pub label: Option<String>,
}

impl DeviceMatch {
    pub fn summary(&self) -> String {
        let ids = match (&self.vendor_id, &self.product_id) {
            (Some(v), Some(p)) => format!(" {v}:{p}"),
            (Some(v), None) => format!(" {v}:????"),
            _ => String::new(),
        };
        let kernel = self
            .kernel
            .as_deref()
            .map(|k| format!(" [{k}]"))
            .unwrap_or_default();
        let label = self
            .label
            .as_deref()
            .map(|l| format!(" ({l})"))
            .unwrap_or_default();
        format!(
            "{sub} {act}{ids}{kernel}{label}",
            sub = self.subsystem,
            act = self.action.as_str(),
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Trigger {
    /// systemd `OnCalendar=` or cron expression.
    OnCalendar(String),
    /// systemd `OnBootSec=` duration or "after boot".
    OnBootSec(String),
    /// Desktop-session login (autostart).
    OnLogin,
    /// udev hardware event — only the systemd-system backend honors this.
    OnDevice(DeviceMatch),
}

impl Trigger {
    pub fn human(&self) -> String {
        match self {
            Trigger::OnCalendar(v) => format!("Calendar: {v}"),
            Trigger::OnBootSec(v) => format!("After boot: {v}"),
            Trigger::OnLogin => "At login".to_string(),
            Trigger::OnDevice(m) => format!("Device: {}", m.summary()),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Trigger::OnCalendar(_) => "OnCalendar",
            Trigger::OnBootSec(_) => "OnBootSec",
            Trigger::OnLogin => "OnLogin",
            Trigger::OnDevice(_) => "OnDevice",
        }
    }

    /// Wire-format value paired with `kind()`. Returns owned `String` because
    /// the device variant must be serialized rather than borrowed.
    pub fn value(&self) -> String {
        match self {
            Trigger::OnCalendar(v) | Trigger::OnBootSec(v) => v.clone(),
            Trigger::OnLogin => String::new(),
            Trigger::OnDevice(m) => serde_json::to_string(m).unwrap_or_default(),
        }
    }

    pub fn from_parts(kind: &str, value: &str) -> Result<Self, SchedulerError> {
        match kind {
            "OnCalendar" => Ok(Trigger::OnCalendar(value.to_string())),
            "OnBootSec" => Ok(Trigger::OnBootSec(value.to_string())),
            "OnLogin" => Ok(Trigger::OnLogin),
            "OnDevice" => {
                let m: DeviceMatch = serde_json::from_str(value)
                    .map_err(|e| SchedulerError::Parse(format!("OnDevice: {e}")))?;
                Ok(Trigger::OnDevice(m))
            }
            other => Err(SchedulerError::Parse(format!(
                "unknown trigger kind: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskOrigin {
    /// Created (or last edited) by this app — has our marker.
    Owned,
    /// Pre-existing on the host (or installed by a package).
    Foreign,
}

impl Default for TaskOrigin {
    fn default() -> Self { TaskOrigin::Owned }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    #[default]
    Persistent,
    DisableAfterRun,
    DeleteAfterRun,
}

impl Lifecycle {
    pub fn as_str(self) -> &'static str {
        match self {
            Lifecycle::Persistent => "persistent",
            Lifecycle::DisableAfterRun => "disable_after_run",
            Lifecycle::DeleteAfterRun => "delete_after_run",
        }
    }
    pub fn from_str(s: &str) -> Self {
        match s {
            "disable_after_run" => Lifecycle::DisableAfterRun,
            "delete_after_run" => Lifecycle::DeleteAfterRun,
            _ => Lifecycle::Persistent,
        }
    }
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub name: String,
    pub command: String,
    pub trigger: Trigger,
    pub enabled: bool,
    pub next_run: Option<String>,
    #[serde(default = "default_scope")]
    pub scope: Scope,
    #[serde(default = "default_backend")]
    pub backend: Backend,
    #[serde(default)]
    pub origin: TaskOrigin,
    #[serde(default)]
    pub lifecycle: Lifecycle,
    /// Snapshot availability hints — populated by `list_tasks`.
    #[serde(default)]
    pub has_snapshot_previous: bool,
    #[serde(default)]
    pub has_snapshot_original: bool,
}

fn default_scope() -> Scope {
    Scope::User
}
fn default_backend() -> Backend {
    Backend::SystemdUser
}



#[derive(Debug, Error)]
pub enum SchedulerError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not determine user config directory")]
    NoConfigDir,
    #[error("invalid task name: {0}")]
    InvalidName(String),
    #[error("systemctl failed: {stderr}")]
    Systemctl { stderr: String },
    #[error("command failed: {0}")]
    CommandFailed(String),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("ipc error: {0}")]
    Ipc(String),
    #[error("permission denied (not root)")]
    NotRoot,
    #[error("backend not supported on this host: {0}")]
    UnsupportedBackend(String),
    #[error("trigger not supported by this backend: {0}")]
    UnsupportedTrigger(String),
    #[error("authentication required or denied")]
    AuthDenied,
    #[error("no snapshot available")]
    NoSnapshot,
    #[error("'{0}' is managed by Task Scheduler itself and cannot be modified from the app")]
    Protected(String),
}

/// Names that Task Scheduler manages for itself. The UI hides the action
/// buttons for these and every mutating adapter method refuses to touch them
/// as a defense-in-depth measure — stopping or rewriting the daemon's own
/// unit from inside the app would break the privileged backend the GUI talks
/// to.
pub const PROTECTED_NAMES: &[&str] = &["task-scheduler-daemon"];

pub fn is_protected_name(name: &str) -> bool {
    PROTECTED_NAMES.iter().any(|n| *n == name)
}

pub fn ensure_not_protected(name: &str) -> Result<(), SchedulerError> {
    if is_protected_name(name) {
        Err(SchedulerError::Protected(name.to_string()))
    } else {
        Ok(())
    }
}


pub trait TaskScheduler {
    fn backend(&self) -> Backend;
    fn capabilities(&self) -> Capabilities;
    fn create_task(&self, task: &Task) -> Result<(), SchedulerError>;
    fn delete_task(&self, name: &str) -> Result<(), SchedulerError>;
    fn list_tasks(&self) -> Result<Vec<Task>, SchedulerError>;
    fn toggle_task(&self, name: &str, enable: bool) -> Result<(), SchedulerError>;
    /// Revert this task to a prior on-disk snapshot. Default: not supported.
    fn revert_task(
        &self,
        _name: &str,
        _target: SnapshotTarget,
    ) -> Result<(), SchedulerError> {
        Err(SchedulerError::NoSnapshot)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotTarget {
    Previous,
    Original,
}

impl SnapshotTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            SnapshotTarget::Previous => "previous",
            SnapshotTarget::Original => "original",
        }
    }
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "previous" => Some(SnapshotTarget::Previous),
            "original" => Some(SnapshotTarget::Original),
            _ => None,
        }
    }
}

pub fn validate_name(name: &str) -> Result<(), SchedulerError> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(SchedulerError::InvalidName(name.to_string()));
    }
    Ok(())
}

/// Heuristic: does this systemd service/timer body look like one we created?
pub fn is_owned_unit_text(text: &str) -> bool {
    text.contains("Description=Task Scheduler:")
        || text.contains("Description=Task Scheduler (device):")
}

/// Heuristic for autostart `.desktop` files.
pub fn is_owned_desktop_text(text: &str) -> bool {
    text.contains("Comment=task-scheduler")
}


pub fn render_service(task: &Task) -> String {
    let mut body = format!(
        "[Unit]\n\
         Description=Task Scheduler: {name}\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         ExecStart={command}\n",
        name = task.name,
        command = task.command,
    );
    match (task.backend, task.lifecycle) {
        (_, Lifecycle::Persistent) => {}
        (Backend::SystemdUser, Lifecycle::DisableAfterRun) => {
            body.push_str(&format!(
                "ExecStartPost=/bin/systemctl --user disable --now {name}.timer\n",
                name = task.name,
            ));
        }
        (Backend::SystemdUser, Lifecycle::DeleteAfterRun) => {
            body.push_str(&format!(
                "ExecStartPost=/bin/sh -c 'systemctl --user disable --now {name}.timer; \
rm -f %h/.config/systemd/user/{name}.service %h/.config/systemd/user/{name}.timer; \
systemctl --user daemon-reload'\n",
                name = task.name,
            ));
        }
        (Backend::SystemdSystem, Lifecycle::DisableAfterRun) => {
            body.push_str(&format!(
                "ExecStartPost=/bin/systemctl disable --now {name}.timer\n",
                name = task.name,
            ));
        }
        (Backend::SystemdSystem, Lifecycle::DeleteAfterRun) => {
            body.push_str(&format!(
                "ExecStartPost=/bin/sh -c 'systemctl disable --now {name}.timer; \
rm -f /etc/systemd/system/{name}.service /etc/systemd/system/{name}.timer; \
systemctl daemon-reload'\n",
                name = task.name,
            ));
        }
        _ => {}
    }
    body
}


pub fn render_timer(task: &Task) -> String {
    let trigger_line = match &task.trigger {
        Trigger::OnCalendar(v) => format!("OnCalendar={v}"),
        Trigger::OnBootSec(v) => format!("OnBootSec={v}"),
        Trigger::OnLogin => "OnBootSec=1min".to_string(), // unreachable for systemd path
        Trigger::OnDevice(_) => "OnBootSec=1min".to_string(), // unreachable — device path has no timer
    };
    format!(
        "[Unit]\n\
         Description=Timer for {name}\n\
         \n\
         [Timer]\n\
         {trigger_line}\n\
         Persistent=true\n\
         Unit={name}.service\n\
         \n\
         [Install]\n\
         WantedBy=timers.target\n",
        name = task.name,
        trigger_line = trigger_line,
    )
}

pub fn parse_trigger(unit: &str) -> Option<Trigger> {
    for line in unit.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("OnCalendar=") {
            return Some(Trigger::OnCalendar(v.to_string()));
        }
        if let Some(v) = line.strip_prefix("OnBootSec=") {
            return Some(Trigger::OnBootSec(v.to_string()));
        }
    }
    None
}

pub fn parse_exec_start(unit: &str) -> Option<String> {
    for line in unit.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("ExecStart=") {
            return Some(v.to_string());
        }
    }
    None
}

/// Translate a small subset of `OnCalendar` expressions to 5-field cron.
/// Returns `None` for anything we can't safely map — caller can fall back to
/// asking the user to provide a literal cron expression prefixed `cron:`.
pub fn systemd_calendar_to_cron(expr: &str) -> Option<String> {
    let e = expr.trim();
    if let Some(rest) = e.strip_prefix("cron:") {
        return Some(rest.trim().to_string());
    }
    match e {
        "hourly" => Some("0 * * * *".into()),
        "daily" | "midnight" => Some("0 0 * * *".into()),
        "weekly" => Some("0 0 * * 0".into()),
        "monthly" => Some("0 0 1 * *".into()),
        _ => {
            // "Mon *-*-* HH:MM[:SS]" — extract HH:MM and the DOW.
            // Very narrow; users can always pass cron: directly.
            let parts: Vec<&str> = e.split_whitespace().collect();
            if parts.len() == 2 {
                let dow = match parts[0] {
                    "Mon" => "1", "Tue" => "2", "Wed" => "3", "Thu" => "4",
                    "Fri" => "5", "Sat" => "6", "Sun" => "0", "*-*-*" => "*",
                    _ => return None,
                };
                let time = parts[1];
                let tparts: Vec<&str> = time.split(':').collect();
                if tparts.len() >= 2 {
                    let h = tparts[0];
                    let m = tparts[1];
                    return Some(format!("{m} {h} * * {dow}"));
                }
            }
            None
        }
    }
}

// --------------------------------------------------------------------------
// udev rules for hardware-attach triggers
// --------------------------------------------------------------------------

pub const UDEV_RULE_PREFIX: &str = "90-task-scheduler-";
pub const UDEV_MARKER_BEGIN: &str = "# >>> task-scheduler:";
pub const UDEV_MARKER_END: &str = "# <<< task-scheduler:";

/// Generate the contents of `/etc/udev/rules.d/90-task-scheduler-<name>.rules`.
/// `service` is the systemd unit (e.g. `task-scheduler-foo.service`).
/// When `enabled` is false the rule body is commented out so udev ignores it.
pub fn render_udev_rule(name: &str, m: &DeviceMatch, service: &str, enabled: bool) -> String {
    let mut parts: Vec<String> = Vec::new();
    // For "change" we also accept "add" — many subsystems only emit `change`
    // after the initial add, and matching both is what users typically expect.
    let action = match m.action {
        DeviceAction::Add => "ACTION==\"add\"".to_string(),
        DeviceAction::Remove => "ACTION==\"remove\"".to_string(),
        DeviceAction::Change => "ACTION==\"add|change\"".to_string(),
    };
    parts.push(action);
    parts.push(format!("SUBSYSTEM==\"{}\"", escape_udev(&m.subsystem)));
    if let Some(v) = &m.vendor_id {
        parts.push(format!("ATTRS{{idVendor}}==\"{}\"", escape_udev(v)));
    }
    if let Some(p) = &m.product_id {
        parts.push(format!("ATTRS{{idProduct}}==\"{}\"", escape_udev(p)));
    }
    if let Some(k) = &m.kernel {
        parts.push(format!("KERNEL==\"{}\"", escape_udev(k)));
    }
    for (k, v) in &m.attrs {
        parts.push(format!(
            "ATTR{{{}}}==\"{}\"",
            escape_udev(k),
            escape_udev(v)
        ));
    }
    parts.push("TAG+=\"systemd\"".to_string());
    parts.push(format!(
        "ENV{{SYSTEMD_WANTS}}+=\"{}\"",
        escape_udev(service)
    ));
    let body = parts.join(", ");
    let line = if enabled { body } else { format!("# DISABLED# {body}") };
    format!(
        "{begin}{name}\n{line}\n{end}{name}\n",
        begin = UDEV_MARKER_BEGIN,
        end = UDEV_MARKER_END,
        name = name,
    )
}

/// Parse a rule file we previously wrote. Returns `(name, match, enabled)`.
pub fn parse_udev_rule(text: &str) -> Option<(String, DeviceMatch, bool)> {
    let mut name: Option<String> = None;
    let mut body_line: Option<&str> = None;
    let mut enabled = true;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix(UDEV_MARKER_BEGIN) {
            name = Some(rest.trim().to_string());
            continue;
        }
        if line.starts_with(UDEV_MARKER_END) {
            break;
        }
        if name.is_some() && body_line.is_none() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("# DISABLED# ") {
                enabled = false;
                body_line = Some(rest);
            } else if !t.is_empty() && !t.starts_with('#') {
                body_line = Some(t);
            }
        }
    }
    let name = name?;
    let line = body_line?;
    let m = parse_rule_body(line)?;
    Some((name, m, enabled))
}

fn parse_rule_body(line: &str) -> Option<DeviceMatch> {
    let mut action = DeviceAction::Add;
    let mut subsystem = String::new();
    let mut vendor_id = None;
    let mut product_id = None;
    let mut kernel = None;
    let mut attrs: Vec<(String, String)> = Vec::new();

    for raw in line.split(',') {
        let part = raw.trim();
        if let Some(v) = strip_kv(part, "ACTION==") {
            action = match v.as_str() {
                "remove" => DeviceAction::Remove,
                "change" | "add|change" => DeviceAction::Change,
                _ => DeviceAction::Add,
            };
        } else if let Some(v) = strip_kv(part, "SUBSYSTEM==") {
            subsystem = v;
        } else if let Some(v) = strip_kv(part, "ATTRS{idVendor}==") {
            vendor_id = Some(v);
        } else if let Some(v) = strip_kv(part, "ATTRS{idProduct}==") {
            product_id = Some(v);
        } else if let Some(v) = strip_kv(part, "KERNEL==") {
            kernel = Some(v);
        } else if let Some((k, v)) = strip_attr(part) {
            attrs.push((k, v));
        }
    }
    if subsystem.is_empty() {
        return None;
    }
    Some(DeviceMatch {
        action,
        subsystem,
        vendor_id,
        product_id,
        kernel,
        attrs,
        label: None,
    })
}

fn strip_kv(part: &str, prefix: &str) -> Option<String> {
    let rest = part.strip_prefix(prefix)?;
    Some(rest.trim().trim_matches('"').to_string())
}

fn strip_attr(part: &str) -> Option<(String, String)> {
    let rest = part.strip_prefix("ATTR{")?;
    let (key, after) = rest.split_once('}')?;
    let val = after.strip_prefix("==")?.trim().trim_matches('"').to_string();
    Some((key.to_string(), val))
}

fn escape_udev(s: &str) -> String {
    // udev rule strings cannot contain unescaped quotes or backslashes.
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Canonical service name used by the udev rule's `SYSTEMD_WANTS`.
pub fn device_service_name(task_name: &str) -> String {
    format!("task-scheduler-device-{task_name}.service")
}

/// Prefix for udev rules that trigger user-scope device tasks.
/// Distinct from `UDEV_RULE_PREFIX` so list scans can tell them apart.
pub const UDEV_USER_RULE_PREFIX: &str = "90-task-scheduler-user-";

/// System service name for a user-scope device task.
/// Lives in /etc/systemd/system/ but carries `User=` so it runs as the owner.
pub fn user_device_service_name(task_name: &str) -> String {
    format!("task-scheduler-user-device-{task_name}.service")
}

/// Render the system bridge service for a user-scope device task.
/// The service is activated by a udev rule (root) but runs the command as
/// `username` (uid) so the user's environment is available. Compatible with
/// any systemd >= 183 (2012).
pub fn render_user_device_service(
    name: &str,
    command: &str,
    username: &str,
    uid: u32,
) -> String {
    format!(
        "[Unit]\n\
         Description=Task Scheduler (user device): {name}\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         User={username}\n\
         Environment=XDG_RUNTIME_DIR=/run/user/{uid}\n\
         ExecStart={command}\n",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_round_trip() {
        let m = DeviceMatch {
            action: DeviceAction::Add,
            subsystem: "usb".into(),
            vendor_id: Some("1d6b".into()),
            product_id: Some("0002".into()),
            kernel: None,
            attrs: Vec::new(),
            label: None,
        };
        let svc = device_service_name("backup");
        let text = render_udev_rule("backup", &m, &svc, true);
        let (name, parsed, enabled) = parse_udev_rule(&text).unwrap();
        assert_eq!(name, "backup");
        assert!(enabled);
        assert_eq!(parsed.subsystem, "usb");
        assert_eq!(parsed.vendor_id.as_deref(), Some("1d6b"));
        assert_eq!(parsed.product_id.as_deref(), Some("0002"));
    }

    #[test]
    fn disabled_rule_parses() {
        let m = DeviceMatch {
            action: DeviceAction::Change,
            subsystem: "block".into(),
            vendor_id: None,
            product_id: None,
            kernel: Some("sd?1".into()),
            attrs: Vec::new(),
            label: None,
        };
        let text = render_udev_rule("x", &m, "task-scheduler-x.service", false);
        let (_, parsed, enabled) = parse_udev_rule(&text).unwrap();
        assert!(!enabled);
        assert_eq!(parsed.kernel.as_deref(), Some("sd?1"));
        assert_eq!(parsed.action, DeviceAction::Change);
    }
}
