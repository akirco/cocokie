//! Typed errors for the whole crate.

use std::path::PathBuf;

/// Every failure mode of [`crate::CookieExtractor`].
///
/// The variants mirror what can realistically go wrong: a missing interpreter,
/// a script that exits non-zero, a timeout, a missing required cookie, and
/// malformed JSON. Detailed text always comes from the script's stderr and
/// never contains cookie values.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The configured Python binary does not exist (spawn returned `ENOENT`).
    #[error("python interpreter not found: {0}")]
    PythonNotFound(String),

    /// The interpreter exists but could not be executed.
    #[error("failed to spawn python: {0}")]
    Spawn(#[source] std::io::Error),

    /// The script exited with a code other than 0 (and not the "missing
    /// required cookie" code 2).
    #[error("extract_cookies.py exited with code {code}: {stderr}")]
    Exit { code: i32, stderr: String },

    /// The child process outlived the configured timeout and was killed.
    #[error(
        "python timed out after {elapsed:?} ({python}); try a longer timeout or chrome+basictext"
    )]
    Timeout {
        python: String,
        elapsed: std::time::Duration,
    },

    /// `--require` names that were not found in the selected cookies.
    #[error("required cookie(s) missing: {0:?}")]
    RequiredMissing(Vec<String>),

    /// The script's stdout was not the JSON the library expects.
    #[error("could not parse JSON from script: {0}")]
    Json(#[from] serde_json::Error),

    /// Writing the embedded script to its cache path failed.
    #[error("failed to materialize embedded script at {path}: {source}")]
    Materialize {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Any other filesystem/IO problem.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Crate-wide result alias.
pub type Result<T> = std::result::Result<T, Error>;
