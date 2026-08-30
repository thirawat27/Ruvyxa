//! Writing build output atomically.
//!
//! A build renders into a staging directory and is moved into place only once
//! it has fully succeeded, so an interrupted or failing build cannot leave a
//! half-written `dist/` that the next `start` would happily serve. If the move
//! fails partway, the previously moved outputs are restored.
//!
//! The commit itself is two moves — the previous outputs into a rollback
//! directory, the staged outputs into their place — and the rollback arm runs
//! only on a returned `Err`. `Drop` does not run on `SIGKILL` and the CLI
//! installs no signal handler, so a process killed between the two moves used
//! to leave `dist/` holding none of the named outputs and the only copy of the
//! previous build inside `.build-rollback-*`, where nothing looked again.
//! [`recover_stranded_build_outputs`] closes that: every commit first sweeps
//! `out_dir` for a rollback directory a dead process left behind and restores
//! from it before deleting it. Deleting one unconditionally is the wrong fix —
//! that is what makes the loss permanent.
//!
//! [`rename_with_windows_retry`] exists because a rename on Windows can fail
//! transiently while a virus scanner or indexer holds a handle on a
//! just-written file; a short bounded retry turns a spurious build failure back
//! into a success without hiding a real one.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Context;

use crate::*;

/// Holds the previous outputs for the duration of one commit.
const BUILD_ROLLBACK_PREFIX: &str = ".build-rollback";
/// Holds a build's outputs until they are ready to be committed.
const BUILD_STAGING_PREFIX: &str = ".build-staging";
/// The journal a commit writes into its rollback directory.
const ROLLBACK_MARKER_FILE: &str = ".ruvyxa-rollback.json";

/// What a rollback directory is holding, and who put it there.
///
/// Written while the previous outputs are already moved aside, which is exactly
/// the window a killed process leaves behind. Recovery reads it rather than
/// guessing: `pid` decides whether the directory belongs to a build still
/// running, and `outputs` names what has to go back.
#[derive(serde::Serialize, serde::Deserialize)]
struct RollbackMarker {
    pid: u32,
    outputs: Vec<String>,
}

/// One `.build-staging-*` or `.build-rollback-*` directory found in `out_dir`.
struct BuildTempDir {
    path: PathBuf,
    /// From the directory name, which is `<prefix>-<pid>-<created at>`. A
    /// directory stranded by a build from before the marker existed still
    /// carries its owner here.
    owner_pid: Option<u32>,
    created_at: u128,
}

pub(crate) fn canonical_route_file(root: &Path, file: &Path) -> PathBuf {
    if file.is_absolute() {
        return ruvyxa_diagnostics::normalized_canonical_path(file);
    }

    let direct = ruvyxa_diagnostics::normalized_canonical_path(file);
    if direct.is_absolute() {
        return direct;
    }
    ruvyxa_diagnostics::normalized_canonical_path(&root.join(file))
}

pub(crate) fn resolve_layout_file(
    root: &Path,
    app_dir: &Path,
    layout_path: &str,
) -> Option<PathBuf> {
    let path = PathBuf::from(layout_path);
    let mut candidates = Vec::new();

    if path.is_absolute() {
        candidates.push(path);
    } else {
        candidates.push(root.join(&path));

        let app_relative = path
            .strip_prefix("app")
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| path.clone());
        candidates.push(app_dir.join(app_relative));
    }

    let mut expanded = Vec::new();
    for candidate in candidates {
        expanded.push(candidate.clone());
        if candidate.extension().is_none() {
            for extension in ["tsx", "jsx", "ts", "js"] {
                expanded.push(candidate.with_extension(extension));
            }
        }
    }

    expanded
        .into_iter()
        .find(|candidate| candidate.is_file())
        .map(|candidate| ruvyxa_diagnostics::normalized_canonical_path(&candidate))
}

pub(crate) fn create_build_staging_dir(out_dir: &Path) -> anyhow::Result<PathBuf> {
    create_build_temp_dir(out_dir, BUILD_STAGING_PREFIX)
}

pub(crate) fn create_build_temp_dir(out_dir: &Path, prefix: &str) -> anyhow::Result<PathBuf> {
    fs::create_dir_all(out_dir)?;
    let created_at = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let temp_dir = out_dir.join(format!("{prefix}-{}-{created_at}", std::process::id()));
    if temp_dir.exists() {
        fs::remove_dir_all(&temp_dir)?;
    }
    fs::create_dir_all(&temp_dir)?;
    Ok(temp_dir)
}

/// Put back what a build killed mid-commit left behind, and sweep what it
/// cannot need.
///
/// For every `.build-rollback-*` directory whose owning process is gone: any
/// output it holds that is **absent** from `out_dir` is restored before the
/// directory is removed. Outputs already present mean that commit got far
/// enough that `out_dir` is whole, so the directory is only removed. A
/// `.build-staging-*` directory from a dead process is removed outright — it
/// holds a partial build nothing can use, and every killed build otherwise
/// leaves one inside `dist/` forever.
///
/// The "is this mine to clean" test is a dead process id, never age. Two
/// `ruvyxa build` invocations against one `dist/` already race, and a sweep
/// keyed on age would turn that race from a lost build into a deleted one.
pub(crate) fn recover_stranded_build_outputs(out_dir: &Path) -> anyhow::Result<()> {
    restore_stranded_rollback_dirs(out_dir)?;
    remove_dead_build_staging_dirs(out_dir)
}

fn restore_stranded_rollback_dirs(out_dir: &Path) -> anyhow::Result<()> {
    // Newest first: if more than one commit was interrupted, the most recent
    // rollback directory holds what `dist/` last served, and an older one must
    // not be restored over it.
    for stranded in build_temp_dirs(out_dir, BUILD_ROLLBACK_PREFIX)? {
        let marker = read_rollback_marker(&stranded.path);
        let owner_pid = marker
            .as_ref()
            .map(|marker| marker.pid)
            .or(stranded.owner_pid);
        if rollback_owner_is_active(owner_pid) {
            continue;
        }

        let held = match marker {
            Some(marker) => marker.outputs,
            // Stranded before the marker was written, or by a build from before
            // the marker existed. What it holds is on disk and does not have to
            // be guessed.
            None => named_build_outputs_present(&stranded.path),
        };
        let missing: Vec<String> = held
            .into_iter()
            .filter(|name| !out_dir.join(name).exists() && stranded.path.join(name).exists())
            .collect();
        restore_named_build_outputs(&stranded.path, out_dir, &missing).with_context(|| {
            format!(
                "failed to restore the previous build from {}",
                stranded.path.display()
            )
        })?;
        // Only after the restore succeeded: the directory is the only copy.
        fs::remove_dir_all(&stranded.path)
            .with_context(|| format!("failed to remove {}", stranded.path.display()))?;
    }

    Ok(())
}

fn remove_dead_build_staging_dirs(out_dir: &Path) -> anyhow::Result<()> {
    for stranded in build_temp_dirs(out_dir, BUILD_STAGING_PREFIX)? {
        // A staging tree lives for a whole build, so one this process owns is
        // the build currently running — unlike a rollback directory, which
        // exists only inside a commit.
        if stranded
            .owner_pid
            .is_none_or(|pid| pid == std::process::id() || process_may_be_running(pid))
        {
            continue;
        }
        fs::remove_dir_all(&stranded.path)
            .with_context(|| format!("failed to remove {}", stranded.path.display()))?;
    }

    Ok(())
}

/// Whether a rollback directory belongs to a commit still in progress.
///
/// A rollback directory exists only inside the body of
/// [`commit_staged_build_outputs`], and this process commits one build at a
/// time, so one recorded by *this* process is left over from an earlier commit
/// that never finished its cleanup — never a concurrent one. An owner that
/// cannot be identified at all reads as active: an unreadable name is not a
/// licence to delete somebody's only copy of a build.
fn rollback_owner_is_active(owner_pid: Option<u32>) -> bool {
    match owner_pid {
        Some(pid) if pid == std::process::id() => false,
        Some(pid) => process_may_be_running(pid),
        None => true,
    }
}

/// The `.build-staging-*` or `.build-rollback-*` directories in `out_dir`,
/// newest first.
///
/// Sorted rather than taken in `read_dir` order, which the filesystem chooses:
/// which of two stranded directories is restored from has to be the same answer
/// on every host.
fn build_temp_dirs(out_dir: &Path, prefix: &str) -> anyhow::Result<Vec<BuildTempDir>> {
    let entries = match fs::read_dir(out_dir) {
        Ok(entries) => entries,
        // Nothing has been built here yet, so there is nothing to recover.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", out_dir.display()));
        }
    };

    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("failed to read {}", out_dir.display()))?;
        if !entry.file_type().is_ok_and(|file_type| file_type.is_dir()) {
            continue;
        }
        let file_name = entry.file_name();
        let Some(suffix) = file_name
            .to_string_lossy()
            .strip_prefix(prefix)
            .and_then(|suffix| suffix.strip_prefix('-'))
            .map(str::to_string)
        else {
            continue;
        };
        // `<prefix>-<pid>-<created at>`, as `create_build_temp_dir` spells it.
        let (owner_pid, created_at) = match suffix.split_once('-') {
            Some((pid, created_at)) => (
                pid.parse::<u32>().ok(),
                created_at.parse::<u128>().unwrap_or_default(),
            ),
            None => (None, 0),
        };
        found.push(BuildTempDir {
            path: entry.path(),
            owner_pid,
            created_at,
        });
    }

    found.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| right.path.cmp(&left.path))
    });
    Ok(found)
}

/// The named build outputs a directory actually holds.
fn named_build_outputs_present(dir: &Path) -> Vec<String> {
    BUILD_OUTPUT_DIRS
        .into_iter()
        .chain(BUILD_OUTPUT_FILES)
        .filter(|name| dir.join(name).exists())
        .map(str::to_string)
        .collect()
}

fn read_rollback_marker(backup_dir: &Path) -> Option<RollbackMarker> {
    let bytes = fs::read(backup_dir.join(ROLLBACK_MARKER_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_rollback_marker(backup_dir: &Path, outputs: &[String]) -> anyhow::Result<()> {
    let marker = RollbackMarker {
        pid: std::process::id(),
        outputs: outputs.to_vec(),
    };
    let bytes = serde_json::to_vec(&marker)?;
    ruvyxa_bundler::atomic_file::write_atomic(&backup_dir.join(ROLLBACK_MARKER_FILE), &bytes)
        .with_context(|| {
            format!(
                "failed to journal the rollback directory {}",
                backup_dir.display()
            )
        })
}

/// Whether a process id may still name a running process.
///
/// Answers conservatively: a platform that cannot be asked reports `true`, so
/// the sweep leaves a directory alone. Leaving one behind costs disk; deleting
/// a running build's working tree costs the build.
fn process_may_be_running(pid: u32) -> bool {
    process_probe::is_running(pid).unwrap_or(true)
}

#[cfg(windows)]
mod process_probe {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    /// What `GetExitCodeProcess` reports for a process that has not exited.
    const STILL_ACTIVE: u32 = 259;
    /// What `OpenProcess` reports for a process id that names nothing. Every
    /// other failure — `ERROR_ACCESS_DENIED` above all — means it exists.
    const ERROR_INVALID_PARAMETER: i32 = 87;

    pub(super) fn is_running(pid: u32) -> Option<bool> {
        // SAFETY: takes an access mask, an inherit flag and a process id by
        // value, and returns a handle or null. Nothing is dereferenced.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return match std::io::Error::last_os_error().raw_os_error() {
                Some(ERROR_INVALID_PARAMETER) => Some(false),
                _ => None,
            };
        }

        // SAFETY: `handle` is a live process handle this call owns, and
        // `CloseHandle` is its only cleanup — which is what `OwnedHandle`
        // performs on drop.
        let owned = unsafe { OwnedHandle::from_raw_handle(handle.cast()) };
        let mut exit_code: u32 = 0;
        // SAFETY: `owned` is a handle opened with
        // `PROCESS_QUERY_LIMITED_INFORMATION`, which is the access
        // `GetExitCodeProcess` needs, and `exit_code` is a writable `u32`.
        let queried =
            unsafe { GetExitCodeProcess(owned.as_raw_handle().cast(), &raw mut exit_code) };
        if queried == 0 {
            return None;
        }
        Some(exit_code == STILL_ACTIVE)
    }
}

#[cfg(target_os = "linux")]
mod process_probe {
    pub(super) fn is_running(pid: u32) -> Option<bool> {
        // `/proc/<pid>` exists for as long as the process has a process-table
        // entry, which includes a zombie waiting to be reaped. Reading a zombie
        // as running only makes the sweep skip a directory it could have
        // cleaned, which is the harmless direction.
        Some(std::path::Path::new(&format!("/proc/{pid}")).exists())
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod process_probe {
    pub(super) fn is_running(pid: u32) -> Option<bool> {
        // A value `pid_t` cannot hold names no process. Refusing it here is
        // also what keeps a negative id away from `kill`, where it means "every
        // process in a group" rather than "this one".
        let Ok(pid) = i32::try_from(pid) else {
            return Some(false);
        };
        if pid <= 0 {
            return Some(false);
        }

        // SAFETY: signal 0 performs `kill`'s existence and permission checks
        // without delivering anything, and both arguments are passed by value.
        if unsafe { libc::kill(pid, 0) } == 0 {
            return Some(true);
        }
        match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::ESRCH) => Some(false),
            // It exists and belongs to another user.
            Some(libc::EPERM) => Some(true),
            _ => None,
        }
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos", target_os = "ios")))]
mod process_probe {
    /// Unknown on this platform, so the sweep never claims a directory.
    pub(super) fn is_running(_pid: u32) -> Option<bool> {
        None
    }
}

pub(crate) fn commit_staged_build_outputs(
    staging_dir: &Path,
    out_dir: &Path,
) -> anyhow::Result<()> {
    // A previous commit may have been killed between its two moves. Whatever it
    // stranded goes back before this commit takes its own backup, so a commit
    // that then fails has a previous build to roll back to.
    recover_stranded_build_outputs(out_dir)?;

    let backup_dir = create_build_temp_dir(out_dir, BUILD_ROLLBACK_PREFIX)?;
    let moved_existing = match open_rollback_window(out_dir, &backup_dir) {
        Ok(moved) => moved,
        Err(error) => {
            let _ = fs::remove_dir_all(&backup_dir);
            return Err(error);
        }
    };
    let commit_result = move_named_build_outputs(staging_dir, out_dir);

    match commit_result {
        Ok(_) => {
            fs::remove_dir_all(&backup_dir)?;
            if staging_dir.exists() {
                fs::remove_dir_all(staging_dir)?;
            }
            Ok(())
        }
        Err(error) => {
            let _ = remove_named_build_outputs(out_dir);
            let rollback_result =
                restore_named_build_outputs(&backup_dir, out_dir, &moved_existing);
            let _ = fs::remove_dir_all(&backup_dir);
            if let Err(rollback_error) = rollback_result {
                return Err(error).with_context(|| {
                    format!(
                        "rollback also failed while restoring previous output: {rollback_error}"
                    )
                });
            }
            Err(error)
        }
    }
}

/// Move the previous outputs aside and journal what was moved.
///
/// Everything after this returns is the window a killed process leaves behind,
/// so the journal is written before the staged outputs go in: it is what tells
/// the next commit whose directory this is and what it holds. A journal that
/// cannot be written is refused rather than skipped — the outputs go back where
/// they were and the build fails loudly, because entering the window without
/// one is how the previous build became unrecoverable in the first place.
fn open_rollback_window(out_dir: &Path, backup_dir: &Path) -> anyhow::Result<Vec<String>> {
    let moved_existing = move_named_build_outputs(out_dir, backup_dir)?;

    if let Err(error) = write_rollback_marker(backup_dir, &moved_existing) {
        if let Err(rollback_error) =
            restore_named_build_outputs(backup_dir, out_dir, &moved_existing)
        {
            return Err(error).with_context(|| {
                format!("rollback also failed while restoring previous output: {rollback_error}")
            });
        }
        return Err(error);
    }

    Ok(moved_existing)
}

pub(crate) fn move_named_build_outputs(from: &Path, to: &Path) -> anyhow::Result<Vec<String>> {
    fs::create_dir_all(to)?;
    let mut moved = Vec::new();

    for name in BUILD_OUTPUT_DIRS.into_iter().chain(BUILD_OUTPUT_FILES) {
        let source = from.join(name);
        if !source.exists() {
            continue;
        }
        let destination = to.join(name);
        if destination.exists() {
            remove_path(&destination)?;
        }
        if let Err(error) = rename_with_windows_retry(&source, &destination) {
            let rollback_result = restore_named_build_outputs(to, from, &moved);
            let mut move_error: anyhow::Error = error.into();
            move_error = move_error.context(format!(
                "failed to move {} to {}",
                source.display(),
                destination.display()
            ));
            if let Err(rollback_error) = rollback_result {
                return Err(move_error).with_context(|| {
                    format!("rollback of partially moved outputs also failed: {rollback_error}")
                });
            }
            return Err(move_error);
        }
        moved.push(name.to_string());
    }

    Ok(moved)
}

pub(crate) fn restore_named_build_outputs(
    backup_dir: &Path,
    out_dir: &Path,
    moved_existing: &[String],
) -> anyhow::Result<()> {
    for name in moved_existing {
        let source = backup_dir.join(name);
        if !source.exists() {
            continue;
        }
        let destination = out_dir.join(name);
        if destination.exists() {
            remove_path(&destination)?;
        }
        rename_with_windows_retry(&source, &destination).with_context(|| {
            format!(
                "failed to restore {} to {}",
                source.display(),
                destination.display()
            )
        })?;
    }

    Ok(())
}

pub(crate) fn rename_with_windows_retry(source: &Path, destination: &Path) -> std::io::Result<()> {
    let mut delay = Duration::from_millis(25);

    for attempt in 0..WINDOWS_RENAME_RETRY_COUNT {
        match fs::rename(source, destination) {
            Ok(()) => return Ok(()),
            Err(error)
                if cfg!(windows)
                    && error.kind() == std::io::ErrorKind::PermissionDenied
                    && attempt + 1 < WINDOWS_RENAME_RETRY_COUNT =>
            {
                std::thread::sleep(delay);
                delay = delay.saturating_mul(2);
            }
            Err(error) => return Err(error),
        }
    }

    unreachable!("the retry loop returns on its final attempt")
}

pub(crate) fn remove_named_build_outputs(out_dir: &Path) -> anyhow::Result<()> {
    for name in BUILD_OUTPUT_DIRS.into_iter().chain(BUILD_OUTPUT_FILES) {
        let path = out_dir.join(name);
        if path.exists() {
            remove_path(&path)?;
        }
    }

    Ok(())
}

pub(crate) fn remove_path(path: &Path) -> anyhow::Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}
