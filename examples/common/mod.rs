//! Glue shared by both examples.
//!
//! The logic that matters (browser spec parsing, Zen detection and cookie
//! replay semantics) lives in the library; this file only adapts the library's
//! dependency-free [`CookieReplay`] output to a reqwest cookie jar, and turns
//! `argv[1]` into a [`BrowserSpec`].

use cocokie::{BrowserSpec, CookieReplay, CookieSet};
use reqwest::cookie::Jar;
use std::sync::Arc;

/// Replay every live cookie into a reqwest jar.
///
/// The library decides which URL each cookie belongs to, so host-only cookies
/// stay on their host and domain cookies keep their scope.
pub fn cookie_jar(set: &CookieSet, base: &str) -> Arc<Jar> {
    let jar = Jar::default();
    for CookieReplay { header, url } in set.replay(base) {
        // The URL comes from the library and is derived from the cookie's own
        // path/domain, so a parse failure would mean a malformed cookie.
        if let Ok(url) = url.parse() {
            jar.add_cookie_str(&header, &url);
        }
    }
    Arc::new(jar)
}

/// `argv[1]` decides the browser: nothing (or `zen`) auto-detects Zen, anything
/// else is parsed as a yt-dlp browser spec.
pub fn spec_from_args() -> BrowserSpec {
    match std::env::args().nth(1).as_deref() {
        None | Some("zen") => BrowserSpec::zen(),
        Some(arg) => BrowserSpec::parse(arg),
    }
}
