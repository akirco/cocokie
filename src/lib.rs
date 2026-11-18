//! Extract cookies on demand from a browser profile or an existing cookie file.
//!
//! The actual browser database access is delegated to
//! [yt-dlp](https://github.com/yt-dlp/yt-dlp)'s cookie extractor, shipped here as
//! an embedded Python script (`vendor/extract_cookies.py`). A call works like
//! this:
//!
//! 1. [`CookieQuery`] describes the source ([`Source::Browser`] or
//!    [`Source::File`]), the domain filter, the field names and the required
//!    names.
//! 2. [`CookieExtractor`] materializes the embedded script into a cache
//!    directory (once per script version) and runs it with `python`.
//! 3. The script prints JSON (`--format json-meta`), which the library parses
//!    into [`Cookie`] / [`CookieSet`], keeping `domain` / `path` / `secure` /
//!    `expires`. [`CookieSet::replay`] then renders those cookies for an HTTP
//!    client's cookie jar without needing one as a dependency.
//!
//! ```no_run
//! use cocokie::{Browser, BrowserSpec, CookieExtractor, CookieQuery};
//!
//! # fn main() -> cocokie::Result<()> {
//! let query = CookieQuery::browser(BrowserSpec::new(Browser::Firefox))
//!     .domain("x.com")
//!     .require(["auth_token", "ct0"]);
//!
//! let set = CookieExtractor::new().extract(&query)?;
//! for cookie in &set.cookies {
//!     println!("{} secure={} ttl={:?}", cookie.name, cookie.secure, cookie.ttl());
//! }
//! # Ok(())
//! # }
//! ```
//!
//! Browser sources need Python >= 3.11 (the `Requires-Python` floor of current
//! yt-dlp) plus an importable `yt_dlp`. File sources need Python >= 3.7 and do
//! not import yt-dlp at all. Values are sensitive: the `Debug` impls of
//! [`Cookie`] and [`CookieSet`] never print cookie values.

pub mod browser;
pub mod error;
pub mod extractor;
pub mod query;
pub mod script;

// Shared test helper (temporary directories); compiled only for tests.
#[cfg(test)]
mod test_util;

pub use browser::{Browser, BrowserSpec};
pub use error::{Error, Result};
pub use extractor::{Cookie, CookieExtractor, CookieInfo, CookieReplay, CookieSet, CookieStatus};
pub use query::{CookieQuery, Source};
