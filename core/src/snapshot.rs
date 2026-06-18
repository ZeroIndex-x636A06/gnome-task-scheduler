//! Per-task on-disk snapshots so we can offer "revert to previous save" and
//! "revert to original (pre-Task-Scheduler) state".
//!
//! Layout (per backend tag, per task name):
//!
//! ```text
//! <root>/<backend-tag>/<task-name>/
//!   original/   # written exactly once, only for foreign tasks the first
//!               # time we touch them
//!   previous/   # overwritten on every save (owned and foreign)
//! ```
//!
//! Each slot is a directory holding raw bytes of every file that defines the
//! task (e.g. a systemd task: `.service` + `.timer`; a cron task: a single
//! `crontab.txt`; an autostart task: `.desktop`).
//!
//! Adapters choose the root path; the daemon uses `/var/lib/task-scheduler`
//! and the GUI's user-scope adapters use `$XDG_DATA_HOME/task-scheduler`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::{Scope, SchedulerError};

/// Conventional snapshot root for a given scope.
pub fn snapshot_root(scope: Scope) -> PathBuf {
    match scope {
        Scope::System => PathBuf::from("/var/lib/task-scheduler/snapshots"),
        Scope::User => {
            let base = std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share"))
                })
                .unwrap_or_else(|| PathBuf::from("/tmp"));
            base.join("task-scheduler/snapshots")
        }
    }
}

pub fn task_snapshot_dir(root: &Path, backend_tag: &str, name: &str) -> PathBuf {
    root.join(backend_tag).join(name)
}

pub fn slot_dir(root: &Path, backend_tag: &str, name: &str, slot: &str) -> PathBuf {
    task_snapshot_dir(root, backend_tag, name).join(slot)
}

pub fn has_slot(root: &Path, backend_tag: &str, name: &str, slot: &str) -> bool {
    let d = slot_dir(root, backend_tag, name, slot);
    d.is_dir() && fs::read_dir(&d).map(|mut it| it.next().is_some()).unwrap_or(false)
}

/// Snapshot a list of source files into the given slot. The destination
/// directory is created (and emptied) first. Missing source files are
/// skipped — typical when one of `.service`/`.timer` pair doesn't exist.
pub fn write_slot(
    root: &Path,
    backend_tag: &str,
    name: &str,
    slot: &str,
    sources: &[&Path],
) -> Result<(), SchedulerError> {
    let dest = slot_dir(root, backend_tag, name, slot);
    if dest.exists() {
        let _ = fs::remove_dir_all(&dest);
    }
    fs::create_dir_all(&dest)?;
    for src in sources {
        if !src.exists() { continue; }
        let Some(file_name) = src.file_name() else { continue };
        let bytes = fs::read(src)?;
        fs::write(dest.join(file_name), bytes)?;
    }
    Ok(())
}

/// Snapshot raw inline content (used by cron, which has no per-task file).
pub fn write_slot_inline(
    root: &Path,
    backend_tag: &str,
    name: &str,
    slot: &str,
    file_name: &str,
    content: &str,
) -> Result<(), SchedulerError> {
    let dest = slot_dir(root, backend_tag, name, slot);
    if dest.exists() {
        let _ = fs::remove_dir_all(&dest);
    }
    fs::create_dir_all(&dest)?;
    fs::write(dest.join(file_name), content)?;
    Ok(())
}

/// Restore files from a slot into the given target directory. Returns the
/// list of file names restored. Returns `NoSnapshot` if the slot is missing.
pub fn restore_slot(
    root: &Path,
    backend_tag: &str,
    name: &str,
    slot: &str,
    target_dir: &Path,
) -> Result<Vec<String>, SchedulerError> {
    let src = slot_dir(root, backend_tag, name, slot);
    if !src.is_dir() {
        return Err(SchedulerError::NoSnapshot);
    }
    fs::create_dir_all(target_dir)?;
    let mut restored = Vec::new();
    let mut empty = true;
    for entry in fs::read_dir(&src)? {
        let entry = entry?;
        empty = false;
        let bytes = fs::read(entry.path())?;
        let file_name = entry.file_name();
        let name_str = file_name.to_string_lossy().to_string();
        fs::write(target_dir.join(&name_str), bytes)?;
        restored.push(name_str);
    }
    if empty {
        return Err(SchedulerError::NoSnapshot);
    }
    Ok(restored)
}

/// Read an inline-snapshotted file (cron-style). Returns `NoSnapshot` if
/// the slot or file is missing.
pub fn restore_slot_inline(
    root: &Path,
    backend_tag: &str,
    name: &str,
    slot: &str,
    file_name: &str,
) -> Result<String, SchedulerError> {
    let p = slot_dir(root, backend_tag, name, slot).join(file_name);
    if !p.exists() {
        return Err(SchedulerError::NoSnapshot);
    }
    Ok(fs::read_to_string(p)?)
}

pub fn purge_task_snapshots(
    root: &Path,
    backend_tag: &str,
    name: &str,
) -> Result<(), SchedulerError> {
    let d = task_snapshot_dir(root, backend_tag, name);
    if d.exists() {
        let _ = fs::remove_dir_all(d);
    }
    Ok(())
}
