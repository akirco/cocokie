//! Test-only helper: a minimal temporary directory.
//!
//! Exists so the crate does not need the `tempfile` dev-dependency just to give
//! tests a scratch directory that cleans itself up.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Disambiguates directories created within the same nanosecond by one process.
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A unique temporary directory removed recursively on drop.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Create the directory, retrying a bounded number of collisions.
    ///
    /// The name combines pid, nanos and a counter, which is enough to keep
    /// parallel test binaries from adopting each other's directories.
    pub fn new() -> std::io::Result<Self> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        for _ in 0..100 {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("cocokie-test-{}-{nanos:x}-{n}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                // Someone else won the name; try the next counter value.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not create a unique temp dir",
        ))
    }

    /// The directory itself.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // Best effort: a failed cleanup must not panic inside a test.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
