# cocokie

A small Rust library that extracts cookies on demand from a browser (or an
existing cookie file), reusing [yt-dlp](https://github.com/yt-dlp/yt-dlp)'s
browser cookie extractor underneath.

- **Only what you ask for** — select by domain + field name instead of dumping
  the whole jar.
- **Safe by default** — `check` / `list` never print values; `Debug` redacts
  them; the library itself writes nothing to disk.
- **Metadata preserved** — `domain` / `path` / `secure` / `expires` survive, so
  replaying a cookie never silently promotes a host-only cookie to a domain
  cookie.
- **Script embedded** — the extractor script is compiled into the library and
  materialized into a cache directory on first use; nothing extra to install.
- **Inspectable** — `check` shows whether each field exists and how long it
  lives; `list` shows which cookies a domain has.

## Requirements

| Source                                       | Needs                                                          |
| -------------------------------------------- | -------------------------------------------------------------- |
| Browser (`--browser` / `BrowserSpec`)        | Python **>= 3.11** + `yt_dlp` importable from that interpreter |
| Cookie file (`--file` / `CookieQuery::file`) | Python **>= 3.7** (does not import yt-dlp)                     |

Why 3.11: the embedded script only needs 3.7 (`dataclass` +
`from __future__ import annotations`), but browser mode has to `import yt_dlp`,
and current yt-dlp (2026.x, observed locally as `2026.10.26`) declares
`Requires-Python >= 3.11`. An older Python can only install an older yt-dlp,
which may or may not work.

The interpreter is probed in this order: `/usr/bin/python3`,
`/usr/local/bin/python3`, `python3`, `python`; `BROWSER_COOKIES_PYTHON`
overrides it. If the wrong interpreter is picked, the error message includes the
path and version the script actually saw.

## As a dependency

```toml
[dependencies]
cocokie = { path = "../cocokie" }
```

## Quick start

### Extract specific fields

```rust
use cocokie::{Browser, BrowserSpec, CookieExtractor, CookieQuery};

fn main() -> cocokie::Result<()> {
    let query = CookieQuery::browser(BrowserSpec::new(Browser::Chrome))
        .domain("bilibili.com")
        .fields(["SESSDATA", "bili_jct", "DedeUserID"])
        .require(["SESSDATA"]);

    let set = CookieExtractor::new().extract(&query)?;

    let sessdata = set.value("SESSDATA").unwrap();
    println!(
        "got {} cookies, SESSDATA is {} chars",
        set.len(),
        sessdata.len()
    );
    Ok(())
}
```

A missing `require` field returns `Error::RequiredMissing` (the script exits
with code 2).

### No fields = every unexpired cookie in the domain

```rust
use cocokie::{Browser, BrowserSpec, CookieExtractor, CookieQuery};

fn main() -> cocokie::Result<()> {
    let query = CookieQuery::browser(BrowserSpec::new(Browser::Firefox))
        .domain("x.com")
        .require(["auth_token", "ct0"]);

    let set = CookieExtractor::new().extract(&query)?;
    println!("got {} cookies", set.len());
    Ok(())
}
```

### Using metadata (recommended for replay)

```rust
use cocokie::{Browser, BrowserSpec, CookieExtractor, CookieQuery};

fn main() -> cocokie::Result<()> {
    let query = CookieQuery::browser(BrowserSpec::new(Browser::Firefox)).domain("x.com");
    let set = CookieExtractor::new().extract(&query)?;

    for cookie in &set.cookies {
        println!(
            "{}: domain={:?} path={} secure={} ttl={:?}",
            cookie.name,
            cookie.domain,
            cookie.path,
            cookie.secure,
            cookie.ttl(), // None for session cookies
        );
    }
    Ok(())
}
```

`set.cookies` is a `Vec<Cookie>`; cookies sharing a name but not a domain/path
are all kept. `set.get(name)` returns the first match.

### Replaying into an HTTP client

The library does not build cookie jars itself (that would mean depending on an
HTTP client); it renders each live cookie with the URL it belongs to:

```rust
use cocokie::{Browser, BrowserSpec, CookieExtractor, CookieQuery};

fn main() -> cocokie::Result<()> {
    let query = CookieQuery::browser(BrowserSpec::new(Browser::Firefox)).domain("x.com");
    let set = CookieExtractor::new().extract(&query)?;

    // (header, url) pairs; expired cookies are already filtered out.
    for replay in set.replay("https://x.com") {
        println!("{} -> {}", replay.header, replay.url);
    }
    Ok(())
}
```

`Cookie::replay_header(now)` does the same for a single cookie. A domain cookie
keeps `Domain=.example.com`; a host-only cookie stays on the base host. Feeding
these into reqwest is three lines:

```rust
use cocokie::CookieSet;
use reqwest::cookie::Jar;
use std::sync::Arc;

fn cookie_jar(set: &CookieSet, base: &str) -> Arc<Jar> {
    let jar = Jar::default();
    for replay in set.replay(base) {
        if let Ok(url) = replay.url.parse() {
            jar.add_cookie_str(&replay.header, &url);
        }
    }
    Arc::new(jar)
}
```

### Inspect / discover fields

```rust
use cocokie::{Browser, BrowserSpec, CookieExtractor, CookieQuery};

fn main() -> cocokie::Result<()> {
    let query = CookieQuery::browser(BrowserSpec::new(Browser::Chrome)).domain("x.com");

    // Status only, no values.
    for status in CookieExtractor::new().check(&query)? {
        println!("{} ok={} {}", status.name, status.ok, status.detail);
        // e.g. auth_token ok=true .x.com / 308.0d left
    }

    // Which cookies exist in the domain (name + domain + TTL, no values).
    for info in CookieExtractor::new().list(&query)? {
        println!(
            "{}\tdomain={:?}\tttl={:?}",
            info.name,
            info.domain,
            info.ttl(), // None for session cookies
        );
    }
    Ok(())
}
```

### Read from a cookie file

```rust
use cocokie::{CookieExtractor, CookieQuery};

fn main() -> cocokie::Result<()> {
    let query = CookieQuery::file("/path/to/cookies.txt")
        .domain("example.com")
        .fields(["session"]);

    let set = CookieExtractor::new().extract(&query)?;
    println!("got {} cookies", set.len());
    Ok(())
}
```

Accepts a Netscape/curl cookie jar or a single `name=value; ...` Cookie header.

### Other options

```rust
use std::time::Duration;
use cocokie::CookieExtractor;

fn main() {
    let _extractor = CookieExtractor::new()
        .with_python("/usr/bin/python3") // explicit interpreter
        .with_script("vendor/extract_cookies.py") // external script, skip materialization
        .with_cache_dir("/tmp/cocokie") // where the embedded script is written
        .with_timeout(Duration::from_secs(30)) // default is 60s
        .without_timeout(); // disable the timeout (not recommended)
}
```

The timeout is on by default: it stops a Chromium keyring dialog from hanging
the call forever.

## Browser notes

- Browsers yt-dlp knows: `chrome`, `chromium`, `firefox`, `edge`, `safari`,
  `brave`, `opera`, `vivaldi`, `whale`.
- Unknown names are passed through verbatim (`Browser::Other`).
- On Linux, Chromium-family browsers may go through the keyring; if it hangs,
  use `chrome+basictext`.
- **Zen** is Firefox-based, but yt-dlp does not know `zen`. Pass the profile's
  absolute path as a Firefox profile:

```rust
use cocokie::{Browser, BrowserSpec};

let spec = BrowserSpec::new(Browser::Firefox)
    .profile("/home/me/.config/zen/<profile>.Default (release)");
```

`BrowserSpec::zen()` does that detection for you: it picks the Zen profile whose
`cookies.sqlite` was modified most recently, falling back to a plain Firefox
spec.

- On Windows, `firefox:C:\Users\me\Profile` parses correctly through
  `BrowserSpec::parse`, which does not treat the drive letter as a browser.

## The underlying script

`vendor/extract_cookies.py` also works standalone:

```bash
# Discover the cookies in a domain (values are never printed)
python3 vendor/extract_cookies.py \
  --browser "firefox:/path/to/profile" \
  --domain bilibili.com --list

# Every unexpired cookie, with metadata
python3 vendor/extract_cookies.py \
  --browser "firefox:/path/to/profile" \
  --domain bilibili.com --fields '*' --format json-meta

# Status only
python3 vendor/extract_cookies.py \
  --browser "firefox:/path/to/profile" \
  --domain bilibili.com --fields 'SESSDATA,bili_jct' --check
```

Formats: `header` (default), `json`, `json-meta`, `value`, `env`.
Exit codes: `0` ok, `1` runtime error, `2` required cookie missing.

## About expiry

- `Cookie::ttl()` is the expiry stored in the browser database minus now;
  session cookies return `None`.
- The server may still revoke the session earlier (password change, logout,
  risk control). TTL only says when the browser will drop the cookie.
- To confirm "does it still work right now", call a site endpoint (see
  `examples/bilibili.rs` and `examples/twitter.rs`).

## Examples

```bash
cargo run --example bilibili   # Bilibili nav API
cargo run --example twitter    # X GraphQL Viewer
```

Both auto-detect a Zen profile by default, and accept an explicit spec:

```bash
cargo run --example bilibili -- firefox
cargo run --example bilibili -- 'chrome:Profile 1'
```

## Tests

```bash
cargo test --all-targets
cargo test --doc
cargo clippy --all-targets -- -D warnings
python3 -m unittest discover -s vendor -p 'test_*.py' -v
python3 -m py_compile vendor/extract_cookies.py
```

CI (`.github/workflows/ci.yml`) runs all of the above.

## Security model

- `check` / `list` output only names, domains and expiry — never values.
- Error text comes from the script's stderr; the script never writes cookie
  values to stderr.
- The library itself writes no cookies to disk and caches none.
- `Cookie`'s `Debug` prints `<redacted>`; `CookieSet`'s `Debug` lists names only.
- Only the script's `--out` writes to disk, with mode `0600`.
