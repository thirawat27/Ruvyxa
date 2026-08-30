//! One durable file publish, used by every cache that writes to disk.
//!
//! Writing a cache entry means the same four steps everywhere: write a temporary
//! file, rename it over the target, survive the platforms where that rename can
//! fail, and never leave the temporary behind. Four call sites had grown their own
//! version of those steps — the bundler's compile cache and graph manifest, the
//! CLI's client-artifact cache and image cache — and they had drifted apart in
//! exactly the places a copy drifts:
//!
//!   - Two named the temporary after a fixed extension, so two writers publishing
//!     the same entry at once used one temporary file for both payloads.
//!   - One skipped removing the temporary when the *first* write failed, so a full
//!     disk left `.tmp` files behind on every attempt.
//!   - One recovered from a failed rename by reading the temporary back and
//!     writing whatever it got — `unwrap_or_default()` on that read, so a
//!     recovery that itself failed replaced a good cache entry with zero bytes.
//!
//! The bytes are already in memory at every call site, which is what makes a
//! shared helper possible: recovery re-writes the buffer it was given rather than
//! reading anything back, so no failure path can publish content that was never
//! passed in.
//!
//! The rename is what makes the publish atomic, so it is not traded away for
//! liveness. This module used to answer *any* rename failure by writing the
//! bytes straight over the target — a truncate-then-write, observable to a
//! concurrent reader as a short file — and justified it by saying every caller
//! writes content-addressed entries, so the loser of a race rewrites identical
//! bytes. That is not true of the callers:
//!
//!   - Two of the four publish fixed-path manifests (`artifact-graph.json`,
//!     `graph-manifest.json`) whose contents differ every build, so the racing
//!     writers do not agree on the bytes at all.
//!   - The one that *is* content-addressed keys on a hash of the **source**,
//!     not of the stored bytes, so a torn entry still sits at the key a later
//!     lookup asks for. `cache::CompileCache` therefore now stores a digest of
//!     the bytes as a header line and treats a mismatch as a miss.
//!
//! So the direct write is reserved for the one failure a rename can never
//! recover from — a target on another device — and every other failure is
//! retried a bounded number of times, because a Windows sharing violation is
//! transient and a genuinely unwritable target should be reported promptly
//! rather than papered over.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Distinguishes temporaries created by the same process at the same instant.
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Temporary path that no other writer can pick.
///
/// The process id separates concurrent builds sharing a cache directory and the
/// counter separates threads inside one build. A name derived only from the
/// target path — the previous `with_extension("json.tmp")` — gave two writers
/// publishing the same entry one temporary between them, so each could rename a
/// file the other was still writing.
fn temporary_path(path: &Path) -> PathBuf {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.{sequence}.tmp", std::process::id()));
    path.with_file_name(name)
}

/// How many times a rename is retried before the failure is reported.
///
/// Three: enough for a Windows sharing violation from a concurrent reader to
/// clear, small enough that a target nothing can ever write to still reports
/// promptly instead of stalling a build.
const RENAME_ATTEMPTS: u32 = 3;

/// Publish `bytes` at `path`, replacing any existing file.
///
/// Readers see either the previous contents or the new ones, never a partial
/// write. A rename that fails is retried up to [`RENAME_ATTEMPTS`] times and
/// then reported; the only failure answered by writing the bytes directly is
/// [`io::ErrorKind::CrossesDevices`], where no number of retries can help and
/// the alternative is losing the entry outright.
///
/// The temporary file is removed on every path out of this function, including
/// the ones that return an error.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic_with(path, bytes, |from, to| fs::rename(from, to))
}

/// [`write_atomic`] with the rename injected, so a test can decide which
/// failure the operating system reports without depending on one.
fn write_atomic_with<R>(path: &Path, bytes: &[u8], mut rename: R) -> io::Result<()>
where
    R: FnMut(&Path, &Path) -> io::Result<()>,
{
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let temporary = temporary_path(path);
    if let Err(error) = fs::write(&temporary, bytes) {
        // The write may have created the file before failing partway.
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }

    let mut last_error = None;
    for attempt in 0..RENAME_ATTEMPTS {
        match rename(&temporary, path) {
            Ok(()) => return Ok(()),
            // A rename cannot move bytes between devices, and retrying says the
            // same thing again. Writing them is the only way to publish at all,
            // and it is the one case where trading atomicity for the entry is
            // better than losing it.
            Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
                let result = fs::write(path, bytes);
                let _ = fs::remove_file(&temporary);
                return result;
            }
            Err(error) => {
                last_error = Some(error);
                if attempt + 1 < RENAME_ATTEMPTS {
                    // A sharing violation clears when the other handle closes.
                    std::thread::sleep(Duration::from_millis(1 << attempt));
                }
            }
        }
    }

    let _ = fs::remove_file(&temporary);
    Err(last_error.unwrap_or_else(|| io::Error::other("rename did not run")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publishes_content_and_leaves_no_temporary_behind() {
        let dir = tempfile::tempdir().expect("temp dir");
        let target = dir.path().join("entry.js");

        write_atomic(&target, b"compiled").expect("write must succeed");

        assert_eq!(fs::read(&target).expect("target exists"), b"compiled");
        assert_eq!(
            leftover_temporaries(dir.path()),
            0,
            "a published entry must leave no .tmp files"
        );
    }

    #[test]
    fn creates_missing_parent_directories() {
        let dir = tempfile::tempdir().expect("temp dir");
        let target = dir.path().join("nested/deeper/entry.js");

        write_atomic(&target, b"compiled").expect("write must create parents");
        assert_eq!(fs::read(&target).expect("target exists"), b"compiled");
    }

    #[test]
    fn replaces_an_existing_entry_without_a_window_of_empty_content() {
        let dir = tempfile::tempdir().expect("temp dir");
        let target = dir.path().join("entry.js");
        fs::write(&target, b"old").expect("seed");

        write_atomic(&target, b"new").expect("replace must succeed");

        assert_eq!(fs::read(&target).expect("target exists"), b"new");
        assert_eq!(leftover_temporaries(dir.path()), 0);
    }

    /// The temporary name must depend on more than the target path, or two
    /// writers publishing one entry share a single temporary file.
    #[test]
    fn temporaries_are_unique_per_call() {
        let path = Path::new("cache/entry.js");
        let first = temporary_path(path);
        let second = temporary_path(path);

        assert_ne!(first, second);
        assert_eq!(
            first.parent(),
            path.parent(),
            "temporary stays beside target"
        );
    }

    /// A directory in place of the target makes both the rename and the direct
    /// write fail. The error must surface and the temporary must still be gone.
    #[test]
    fn a_failed_publish_reports_the_error_and_still_cleans_up() {
        let dir = tempfile::tempdir().expect("temp dir");
        let target = dir.path().join("entry.js");
        fs::create_dir(&target).expect("occupy the target path with a directory");

        assert!(
            write_atomic(&target, b"compiled").is_err(),
            "publishing over a directory cannot succeed"
        );
        assert_eq!(
            leftover_temporaries(dir.path()),
            0,
            "a failed publish must not leave a .tmp file behind"
        );
    }

    /// A cross-device target is the one failure a rename can never recover
    /// from, so it — and only it — still publishes by writing the bytes.
    #[test]
    fn a_cross_device_target_still_publishes() {
        let dir = tempfile::tempdir().expect("temp dir");
        let target = dir.path().join("entry.js");
        let mut attempts = 0;

        write_atomic_with(&target, b"compiled", |_, _| {
            attempts += 1;
            Err(io::Error::from(io::ErrorKind::CrossesDevices))
        })
        .expect("a cross-device rename must fall back rather than lose the entry");

        assert_eq!(attempts, 1, "retrying a cross-device rename cannot help");
        assert_eq!(fs::read(&target).expect("target exists"), b"compiled");
        assert_eq!(leftover_temporaries(dir.path()), 0);
    }

    /// Every other failure is transient until proven otherwise — a Windows
    /// sharing violation clears when the other handle closes. It must be
    /// retried, never answered with a truncating write over the target.
    #[test]
    fn a_transient_rename_failure_is_retried_and_then_succeeds() {
        let dir = tempfile::tempdir().expect("temp dir");
        let target = dir.path().join("entry.js");
        fs::write(&target, b"previous").expect("seed");
        let mut attempts = 0;

        write_atomic_with(&target, b"compiled", |from, to| {
            attempts += 1;
            if attempts < 2 {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
            }
            fs::rename(from, to)
        })
        .expect("a transient failure must be retried");

        assert_eq!(attempts, 2);
        assert_eq!(fs::read(&target).expect("target exists"), b"compiled");
        assert_eq!(leftover_temporaries(dir.path()), 0);
    }

    /// A rename that never succeeds must report, not fall back. The fallback is
    /// a truncate-then-write, and two of this module's callers publish
    /// fixed-path manifests whose bytes differ every build — so the racing
    /// writers do not agree on the content and a reader can see a short file.
    #[test]
    fn a_persistent_rename_failure_reports_instead_of_truncating_the_target() {
        let dir = tempfile::tempdir().expect("temp dir");
        let target = dir.path().join("entry.js");
        fs::write(&target, b"previous").expect("seed");
        let mut attempts = 0;

        let error = write_atomic_with(&target, b"compiled", |_, _| {
            attempts += 1;
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        })
        .expect_err("a rename that never succeeds must surface");

        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(attempts, RENAME_ATTEMPTS as usize);
        assert_eq!(
            fs::read(&target).expect("target exists"),
            b"previous",
            "the previous entry must survive a failed publish intact"
        );
        assert_eq!(leftover_temporaries(dir.path()), 0);
    }

    fn leftover_temporaries(dir: &Path) -> usize {
        fs::read_dir(dir)
            .expect("readable directory")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .count()
    }
}
