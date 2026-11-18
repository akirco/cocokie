//! Materializing the embedded Python script on disk.
//!
//! The script is compiled into the library with [`include_str!`]. At run time it
//! must exist as a real file because `python` is executed with a path argument.
//! The file name carries a hash of the script contents, so an upgraded library
//! never reuses a stale script and concurrent runs never read a half-written one.

use crate::error::{Error, Result};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The extractor script, compiled into the binary.
pub const SCRIPT: &str = include_str!("../vendor/extract_cookies.py");

/// How long an older script version may sit idle before it is deleted.
/// Short enough to clean up versions left over from development, long enough
/// that a concurrently starting process does not lose the file it just wrote.
const STALE_AFTER: Duration = Duration::from_secs(600);

/// Hash used in the cache file name.
fn content_hash() -> u64 {
    fnv1a(SCRIPT.as_bytes())
}

/// FNV-1a 64-bit.
///
/// Deliberately not a cryptographic hash: this only names a cache file, and
/// FNV-1a is dependency-free and stable across Rust releases, so the file name
/// does not churn when the compiler or a crate version changes.
fn fnv1a(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes.iter().fold(OFFSET, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(PRIME)
    })
}

/// Write the embedded script into a directory and return its path.
///
/// Without an explicit `cache_dir`, the platform cache directory is tried first;
/// if it is not writable (read-only container, permissions) the temporary
/// directory is used instead of failing outright. An explicitly supplied
/// `cache_dir` is never silently replaced.
pub fn materialize(cache_dir: Option<&Path>) -> Result<PathBuf> {
    if let Some(dir) = cache_dir {
        return materialize_in(dir);
    }

    let mut last_error = None;
    for dir in fallback_dirs() {
        match materialize_in(&dir) {
            Ok(path) => return Ok(path),
            Err(e) => last_error = Some(e),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        Error::Io(std::io::Error::other(
            "no writable directory for extract_cookies.py",
        ))
    }))
}

/// Materialize into exactly this directory.
fn materialize_in(dir: &Path) -> Result<PathBuf> {
    let hash = content_hash();
    let path = dir.join(format!("extract_cookies-{hash:016x}.py"));
    if !path.exists() {
        std::fs::create_dir_all(dir)?;

        // Write to a per-process temp name first, then rename, so no reader can
        // observe a partially written script.
        let tmp = dir.join(format!(
            ".extract_cookies.{}.{hash:016x}.tmp",
            std::process::id()
        ));
        if let Err(e) = std::fs::write(&tmp, SCRIPT) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        // rename over an existing target fails on Windows, so remove it first.
        let _ = std::fs::remove_file(&path);
        std::fs::rename(&tmp, &path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            Error::Materialize {
                path: path.clone(),
                source: e,
            }
        })?;
    }

    prune_stale(dir, &path, STALE_AFTER);
    Ok(path)
}

/// Delete older script versions left in the cache after the content changed.
///
/// Only files this library owns are considered (`extract_cookies-<hash>.py` and
/// `.extract_cookies.*.tmp`), and only when idle for `max_age`, so a version
/// another process is still using is left alone. `keep` is never deleted.
fn prune_stale(dir: &Path, keep: &Path, max_age: Duration) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return; // Not our directory to worry about.
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path == keep {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let ours = (name.starts_with("extract_cookies-") && name.ends_with(".py"))
            || (name.starts_with(".extract_cookies.") && name.ends_with(".tmp"));
        if !ours {
            continue;
        }
        // Ignore errors here: pruning is opportunistic and never fatal.
        let idle = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age >= max_age);
        if idle {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Directories to try, in order, when the caller did not pick one.
fn fallback_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(cache) = dirs::cache_dir() {
        dirs.push(cache.join("browser-cookies"));
    }
    dirs.push(std::env::temp_dir().join(format!("cocokie-{}", user_tag())));
    dirs
}

/// Namespace the temp directory by user, since `/tmp` is usually shared.
fn user_tag() -> String {
    let raw = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();
    // Keep the tag filesystem-safe; anything unusual collapses to "user".
    let tag: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .collect();
    if tag.is_empty() { "user".into() } else { tag }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    /// Two calls must reuse the same file instead of rewriting it.
    #[test]
    fn materializes_and_reuses_same_path() {
        let dir = TempDir::new().unwrap();

        let first = materialize(Some(dir.path())).unwrap();
        let second = materialize(Some(dir.path())).unwrap();

        assert_eq!(first, second);
        assert!(first.exists());
        assert_eq!(std::fs::read_to_string(first).unwrap(), SCRIPT);
    }

    /// Only our stale files go away; the active script and foreign files stay.
    #[test]
    fn prune_removes_only_our_stale_files() {
        let dir = TempDir::new().unwrap();
        let keep = materialize(Some(dir.path())).unwrap();
        let old = dir.path().join("extract_cookies-0000000000000000.py");
        let tmp = dir.path().join(".extract_cookies.1.0000000000000000.tmp");
        let unrelated = dir.path().join("notes.txt");
        std::fs::write(&old, "old").unwrap();
        std::fs::write(&tmp, "half").unwrap();
        std::fs::write(&unrelated, "keep me").unwrap();

        prune_stale(dir.path(), &keep, Duration::ZERO);

        assert!(!old.exists());
        assert!(!tmp.exists());
        assert!(keep.exists());
        assert!(unrelated.exists());
    }

    /// Guards the hash implementation against accidental edits.
    #[test]
    fn fnv1a_matches_known_vector() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"hello"), 0xa430_d846_80aa_bd0b);
    }
}
