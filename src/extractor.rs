use crate::error::{Error, Result};
use crate::query::CookieQuery;
use crate::script;
use std::io::Read;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Default timeout: prevents an interactive prompt (e.g. a Chromium keyring
/// dialog) from hanging the call forever.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// Runs the embedded script and turns its output into structured cookies.
///
/// Cheap to clone; all fields are plain paths/durations.
#[derive(Clone)]
pub struct CookieExtractor {
    python: PathBuf,
    script: Option<PathBuf>,
    cache_dir: Option<PathBuf>,
    timeout: Option<Duration>,
}

/// One cookie. The value is sensitive: `Debug` never prints it.
#[derive(Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    /// A leading dot means a domain cookie; otherwise host-only.
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default = "default_path")]
    pub path: String,
    #[serde(default)]
    pub secure: bool,
    /// Unix seconds; `None` means a session cookie.
    #[serde(default)]
    pub expires: Option<i64>,
}

fn default_path() -> String {
    "/".into()
}

impl Cookie {
    pub fn expires_at(&self) -> Option<SystemTime> {
        let expires = u64::try_from(self.expires?).ok()?;
        UNIX_EPOCH.checked_add(Duration::from_secs(expires))
    }

    /// Remaining lifetime. `None` for session cookies, `Some(Duration::ZERO)`
    /// once expired.
    ///
    /// This is only the expiry recorded in the browser database; the server may
    /// still revoke the session earlier.
    pub fn ttl(&self) -> Option<Duration> {
        ttl_at(self.expires, now_secs())
    }

    pub fn is_expired(&self) -> bool {
        expired_at(self.expires, now_secs())
    }

    /// Render this cookie for replaying into an HTTP client's cookie jar.
    ///
    /// Produces `name=value; Path=...` plus `Domain=` only when the cookie really
    /// is a domain cookie, `Secure` when set, and `Max-Age=` for persistent
    /// cookies. Returns `None` once expired at `now` (Unix seconds), so callers
    /// never send a dead credential or a negative `Max-Age`.
    pub fn replay_header(&self, now: i64) -> Option<String> {
        if expired_at(self.expires, now) {
            return None;
        }

        let mut header = format!("{}={}; Path={}", self.name, self.value, self.path);
        if let Some(domain) = self.domain.as_deref().filter(|d| d.starts_with('.')) {
            header.push_str(&format!("; Domain={domain}"));
        }
        if self.secure {
            header.push_str("; Secure");
        }
        if let Some(expires) = self.expires {
            header.push_str(&format!("; Max-Age={}", expires.saturating_sub(now)));
        }
        Some(header)
    }
}

/// One cookie rendered for an HTTP client's cookie jar.
///
/// The crate stays free of an HTTP dependency: `url` is a plain string that the
/// caller parses (e.g. `reqwest::Url`) before handing it to
/// [`CookieReplay::header`]. Domain/path/secure handling is already correct,
/// which is the part that is easy to get wrong by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CookieReplay {
    /// `name=value; Path=...; Domain=...; Secure; Max-Age=...`
    pub header: String,
    /// Absolute URL this cookie applies to, e.g. `https://example.com/`.
    pub url: String,
}

/// Remaining lifetime at an explicit `now` (Unix seconds).
fn ttl_at(expires: Option<i64>, now: i64) -> Option<Duration> {
    let remaining = expires?.saturating_sub(now);
    Some(Duration::from_secs(remaining.max(0) as u64))
}

/// Expiry check at an explicit `now` (Unix seconds).
fn expired_at(expires: Option<i64>, now: i64) -> bool {
    matches!(expires, Some(expires) if expires <= now)
}

/// Current time as Unix seconds; `0` if the clock is before the epoch.
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Hand-written `Debug` that redacts the value.
impl std::fmt::Debug for Cookie {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cookie")
            .field("name", &self.name)
            .field("value", &"<redacted>")
            .field("domain", &self.domain)
            .field("path", &self.path)
            .field("secure", &self.secure)
            .field("expires", &self.expires)
            .finish()
    }
}

/// Sensitive values are never printed: `Debug` lists names only.
#[derive(Clone, PartialEq, Eq)]
pub struct CookieSet {
    pub cookies: Vec<Cookie>,
    pub missing: Vec<String>,
}

impl CookieSet {
    /// First cookie with this name (matching the script's selection order, so
    /// the first entry wins when a name exists in several domains/paths).
    pub fn get(&self, name: &str) -> Option<&Cookie> {
        self.cookies.iter().find(|c| c.name == name)
    }

    pub fn value(&self, name: &str) -> Option<&str> {
        self.get(name).map(|c| c.value.as_str())
    }

    pub fn len(&self) -> usize {
        self.cookies.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }

    /// Render every cookie that is still valid at `now` (Unix seconds) for
    /// replaying into an HTTP client's cookie jar.
    ///
    /// `base` is a full URL such as `https://x.com`; its host is the fallback
    /// scope for cookies with no `domain`. Each returned [`CookieReplay`] carries the exact
    /// URL it belongs to, so a domain cookie is offered to `example.com` while a
    /// host-only cookie stays on the page's host.
    pub fn replay_at(&self, base: &str, now: i64) -> Vec<CookieReplay> {
        let base_host = host_of(base);
        self.cookies
            .iter()
            .filter_map(|cookie| {
                let header = cookie.replay_header(now)?;
                let host = cookie
                    .domain
                    .as_deref()
                    .map(|d| d.trim_start_matches('.'))
                    .filter(|d| !d.is_empty())
                    .unwrap_or(base_host);
                let scheme = if cookie.secure { "https" } else { "http" };
                Some(CookieReplay {
                    header,
                    url: format!("{scheme}://{host}{}", cookie.path),
                })
            })
            .collect()
    }

    /// [`CookieSet::replay_at`] using the current time.
    pub fn replay(&self, base: &str) -> Vec<CookieReplay> {
        self.replay_at(base, now_secs())
    }
}

/// Extract the host from a URL string without a URL parser.
///
/// `https://user@host:8443/path` → `host:8443`. Falls back to `localhost` so the
/// generated URL is always syntactically usable.
fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let rest = rest.rsplit_once('@').map_or(rest, |(_, rest)| rest);
    rest.split(['/', '?', '#'])
        .next()
        .filter(|host| !host.is_empty())
        .unwrap_or("localhost")
}

impl std::fmt::Debug for CookieSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CookieSet")
            .field(
                "names",
                &self.cookies.iter().map(|c| &c.name).collect::<Vec<_>>(),
            )
            .field("missing", &self.missing)
            .finish()
    }
}

/// Result of [`CookieExtractor::check`]: whether each requested name exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CookieStatus {
    pub name: String,
    /// The field exists and has a non-empty value.
    pub ok: bool,
    /// For example `.spotify.com / 178.3d left`; empty when missing.
    pub detail: String,
}

/// Result of [`CookieExtractor::list`]: cookies present in the domain, no values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CookieInfo {
    pub name: String,
    /// A leading dot means a domain cookie; otherwise host-only.
    pub domain: Option<String>,
    /// Unix seconds; `None` means a session cookie.
    pub expires: Option<i64>,
}

impl CookieInfo {
    /// Remaining lifetime; `None` for session cookies, `Some(Duration::ZERO)`
    /// once expired.
    pub fn ttl(&self) -> Option<Duration> {
        ttl_at(self.expires, now_secs())
    }

    pub fn is_expired(&self) -> bool {
        expired_at(self.expires, now_secs())
    }
}

/// Defaults: probed interpreter, embedded script, platform cache directory and a
/// 60-second timeout.
impl Default for CookieExtractor {
    fn default() -> Self {
        Self {
            python: default_python(),
            script: None,
            cache_dir: None,
            timeout: Some(DEFAULT_TIMEOUT),
        }
    }
}

impl CookieExtractor {
    /// Same as [`CookieExtractor::default`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Use a specific Python interpreter (e.g. `/usr/bin/python3`).
    pub fn with_python(mut self, p: impl Into<PathBuf>) -> Self {
        self.python = p.into();
        self
    }

    /// Use an external script instead of materializing the embedded one.
    pub fn with_script(mut self, p: impl Into<PathBuf>) -> Self {
        self.script = Some(p.into());
        self
    }

    /// Directory to write the embedded script into; always used as given.
    pub fn with_cache_dir(mut self, p: impl Into<PathBuf>) -> Self {
        self.cache_dir = Some(p.into());
        self
    }

    /// Change the Python timeout (default 60s); zero means "time out immediately".
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Disable the timeout and wait indefinitely (only when no prompt is possible).
    pub fn without_timeout(mut self) -> Self {
        self.timeout = None;
        self
    }

    /// Extract cookies. With empty `q.fields`, returns every unexpired cookie in
    /// the requested domain.
    pub fn extract(&self, q: &CookieQuery) -> Result<CookieSet> {
        let script = self.ensure_script()?;
        let (code, stdout, stderr) = self.run(&script, &q.to_args())?;

        match code {
            0 => {}
            2 => {
                if let Some(missing) = parse_missing_required(&stderr) {
                    return Err(Error::RequiredMissing(missing));
                }
                return Err(Error::Exit { code, stderr });
            }
            c => return Err(Error::Exit { code: c, stderr }),
        }

        let cookies: Vec<Cookie> = serde_json::from_str(&stdout)?;
        let missing = q
            .fields
            .iter()
            .filter(|f| !cookies.iter().any(|c| c.name == **f))
            .cloned()
            .collect();
        Ok(CookieSet { cookies, missing })
    }

    /// Corresponds to `--list`: names, domains and expiry in the domain, no
    /// values. Expired cookies are excluded unless
    /// [`CookieQuery::allow_expired`] is set.
    pub fn list(&self, q: &CookieQuery) -> Result<Vec<CookieInfo>> {
        let script = self.ensure_script()?;
        let mut args = q.to_args();
        args.push("--list".into());
        let (code, stdout, stderr) = self.run(&script, &args)?;
        if code != 0 {
            return Err(Error::Exit { code, stderr });
        }
        Ok(parse_list_entries(&stdout))
    }

    /// Corresponds to `--check`: reports status only, never values.
    pub fn check(&self, q: &CookieQuery) -> Result<Vec<CookieStatus>> {
        let script = self.ensure_script()?;
        let mut args = q.to_args();
        args.push("--check".into());
        let (code, stdout, stderr) = self.run(&script, &args)?;
        if code == 2 {
            if let Some(missing) = parse_missing_required(&stderr) {
                return Err(Error::RequiredMissing(missing));
            }
            return Err(Error::Exit { code, stderr });
        }
        if code != 0 {
            return Err(Error::Exit { code, stderr });
        }
        Ok(parse_check_table(&stdout))
    }

    /// Return the script path, materializing the embedded one if needed.
    fn ensure_script(&self) -> Result<PathBuf> {
        if let Some(p) = &self.script {
            return Ok(p.clone());
        }
        script::materialize(self.cache_dir.as_deref())
    }

    /// Run the script and return `(exit code, stdout, stderr)`.
    ///
    /// With a timeout configured the child gets piped stdout/stderr that are
    /// drained on separate threads, while the main thread polls for exit. That
    /// avoids the classic deadlock where a chatty child fills the pipe buffer
    /// and blocks forever in `wait`.
    fn run(
        &self,
        script: &std::path::Path,
        args: &[std::ffi::OsString],
    ) -> Result<(i32, String, String)> {
        let mut command = Command::new(&self.python);
        command
            .arg(script)
            .args(args)
            .stdin(std::process::Stdio::null());

        let Some(timeout) = self.timeout else {
            let out = command.output().map_err(|e| self.spawn_error(e))?;
            return Ok((
                out.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&out.stdout).into_owned(),
                String::from_utf8_lossy(&out.stderr).into_owned(),
            ));
        };

        let mut child = command
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| self.spawn_error(e))?;

        // Pipes must be drained while waiting, or the child can block once the
        // buffer fills up.
        let stdout = child.stdout.take().expect("stdout was piped above");
        let stderr = child.stderr.take().expect("stderr was piped above");
        let out_handle = std::thread::spawn(move || read_all(stdout));
        let err_handle = std::thread::spawn(move || read_all(stderr));

        let deadline = Instant::now() + timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = out_handle.join();
                    let _ = err_handle.join();
                    return Err(Error::Timeout {
                        python: self.python.display().to_string(),
                        elapsed: timeout,
                    });
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(Error::Spawn(e));
                }
            }
        };

        Ok((
            status.code().unwrap_or(-1),
            out_handle.join().unwrap_or_default(),
            err_handle.join().unwrap_or_default(),
        ))
    }

    /// Turn a spawn failure into `PythonNotFound` when the binary is missing.
    fn spawn_error(&self, e: std::io::Error) -> Error {
        match e.kind() {
            std::io::ErrorKind::NotFound => {
                Error::PythonNotFound(self.python.display().to_string())
            }
            _ => Error::Spawn(e),
        }
    }
}

/// Read a stream to EOF, tolerating invalid UTF-8 (lossy conversion).
fn read_all(mut reader: impl Read) -> String {
    let mut buf = Vec::new();
    let _ = reader.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

/// Parse the script's `required cookie(s) missing: a, b` stderr line.
fn parse_missing_required(stderr: &str) -> Option<Vec<String>> {
    stderr.lines().find_map(|line| {
        let idx = line.find("required cookie(s) missing: ")?;
        let rest = &line[idx + "required cookie(s) missing: ".len()..];
        Some(
            rest.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect(),
        )
    })
}

/// Parse `--list` output: `name<TAB>domain<TAB>expires`, where `-` means absent
/// and a session cookie has no expiry.
fn parse_list_entries(stdout: &str) -> Vec<CookieInfo> {
    stdout
        .lines()
        .filter_map(|line| {
            let mut it = line.split('\t');
            let name = it.next()?.trim();
            if name.is_empty() {
                return None;
            }
            let domain = it.next().unwrap_or("-");
            let expires = it.next().unwrap_or("-").trim();
            Some(CookieInfo {
                name: name.to_string(),
                domain: (domain != "-" && !domain.is_empty()).then(|| domain.to_string()),
                expires: expires.parse().ok(),
            })
        })
        .collect()
}

/// Parse `--check` output: `name<TAB>ok|MISSING<TAB>domain / ttl`.
fn parse_check_table(stdout: &str) -> Vec<CookieStatus> {
    stdout
        .lines()
        .filter_map(|line| {
            let mut it = line.split_whitespace();
            let name = it.next()?.to_string();
            let status = it.next()?;
            let detail = it.collect::<Vec<_>>().join(" ");
            Some(CookieStatus {
                name,
                ok: status == "ok",
                detail,
            })
        })
        .collect()
}

/// Locate a usable Python: `BROWSER_COOKIES_PYTHON` wins, then a short list of
/// well-known absolute paths, then anything on `PATH`.
fn default_python() -> PathBuf {
    if let Some(p) = std::env::var_os("BROWSER_COOKIES_PYTHON") {
        return PathBuf::from(p);
    }
    #[cfg(windows)]
    let candidates: &[&str] = &["py.exe", "python.exe", "python3.exe"];
    #[cfg(not(windows))]
    let candidates: &[&str] = &[
        "/usr/bin/python3",
        "/usr/local/bin/python3",
        "python3",
        "python",
    ];

    for c in candidates {
        let ok = if c.contains('/') || c.contains('\\') {
            std::path::Path::new(c).is_file()
        } else {
            // Minimal PATH probe: no shelling out, no `which`.
            std::env::var_os("PATH")
                .map(|paths| std::env::split_paths(&paths).any(|d| d.join(c).is_file()))
                .unwrap_or(false)
        };
        if ok {
            return PathBuf::from(c);
        }
    }
    PathBuf::from("python3")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::CookieQuery;
    use crate::test_util::TempDir;

    /// Shared fixture: two live cookies, one expired, one from another domain.
    const JAR: &str = "# Netscape HTTP Cookie File\n\
.example.com\tTRUE\t/\tFALSE\t4102444800\talive\t1\n\
.example.com\tTRUE\t/\tFALSE\t1\texpired\tx\n\
.example.com\tTRUE\t/\tFALSE\t0\tsession\ts\n\
.other.com\tTRUE\t/\tFALSE\t4102444800\tforeign\tf\n";

    /// Extractor pointed at the repository's script, skipping materialization.
    fn extractor(_dir: &TempDir) -> CookieExtractor {
        CookieExtractor::new().with_script(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/vendor/extract_cookies.py"
        ))
    }

    /// Write the shared fixture into the temp dir and return its path.
    fn jar(dir: &TempDir) -> PathBuf {
        let path = dir.path().join("cookies.txt");
        std::fs::write(&path, JAR).unwrap();
        path
    }

    /// No fields = every unexpired cookie in the domain, with metadata preserved.
    #[test]
    fn empty_fields_extracts_everything_unexpired() {
        let dir = TempDir::new().unwrap();
        let ex = extractor(&dir);
        let query = CookieQuery::file(jar(&dir)).domain("example.com");

        let set = ex.extract(&query).unwrap();

        assert_eq!(set.len(), 2, "{set:?}");
        assert_eq!(set.value("alive"), Some("1"));
        assert_eq!(set.value("session"), Some("s"));
        assert!(set.get("expired").is_none());
        assert!(set.get("foreign").is_none());
        assert!(set.missing.is_empty());

        let alive = set.get("alive").unwrap();
        assert_eq!(alive.domain.as_deref(), Some(".example.com"));
        assert_eq!(alive.path, "/");
        assert!(!alive.secure);
        assert_eq!(alive.expires, Some(4_102_444_800));
    }

    /// `list` exposes names/domains/expiry but never values.
    #[test]
    fn list_reports_names_without_values() {
        let dir = TempDir::new().unwrap();
        let ex = extractor(&dir);
        let query = CookieQuery::file(jar(&dir)).domain("example.com");

        let list = ex.list(&query).unwrap();
        let names: Vec<_> = list.iter().map(|s| s.name.as_str()).collect();

        assert_eq!(names, ["alive", "session"]);
        assert!(list.iter().all(|c| !c.is_expired()));
        assert_eq!(list[0].domain.as_deref(), Some(".example.com"));
        assert_eq!(list[0].expires, Some(4_102_444_800));
        assert_eq!(list[1].expires, None);
    }

    /// Expired entries only appear when explicitly allowed.
    #[test]
    fn list_can_include_expired() {
        let dir = TempDir::new().unwrap();
        let ex = extractor(&dir);
        let query = CookieQuery::file(jar(&dir))
            .domain("example.com")
            .allow_expired(true);

        let list = ex.list(&query).unwrap();
        let expired = list.iter().find(|s| s.name == "expired").unwrap();

        assert!(expired.is_expired());
    }

    /// Explicit fields still work and report the ones that were not found.
    #[test]
    fn explicit_fields_still_work_and_report_missing() {
        let dir = TempDir::new().unwrap();
        let ex = extractor(&dir);
        let query = CookieQuery::file(jar(&dir))
            .domain("example.com")
            .fields(["alive", "absent"]);

        let set = ex.extract(&query).unwrap();

        assert_eq!(set.len(), 1);
        assert_eq!(set.missing, ["absent"]);
    }

    /// A script that sleeps past the timeout must be killed and reported.
    #[test]
    fn times_out_on_slow_script() {
        let dir = TempDir::new().unwrap();
        let slow = dir.path().join("slow.py");
        std::fs::write(&slow, "import time\ntime.sleep(30)\n").unwrap();
        let ex = CookieExtractor::new()
            .with_script(slow)
            .with_timeout(std::time::Duration::from_millis(200));
        let query = CookieQuery::file(dir.path().join("unused.txt"));

        let err = ex.extract(&query).unwrap_err();

        assert!(matches!(err, Error::Timeout { .. }), "{err:?}");
    }

    /// `check` reports per-name status and detail.
    #[test]
    fn check_reports_ok_and_missing() {
        let dir = TempDir::new().unwrap();
        let ex = extractor(&dir);
        let query = CookieQuery::file(jar(&dir))
            .domain("example.com")
            .fields(["alive", "absent"]);

        let statuses = ex.check(&query).unwrap();

        assert_eq!(statuses.len(), 2);
        assert_eq!(statuses[0].name, "alive");
        assert!(statuses[0].ok);
        assert!(
            statuses[0].detail.contains("example.com"),
            "{:?}",
            statuses[0]
        );
        assert_eq!(statuses[1].name, "absent");
        assert!(!statuses[1].ok);
    }

    /// Fresh, expired and session cookies each behave as documented.
    #[test]
    fn ttl_and_expiry() {
        let now = now_secs();

        let fresh = cookie_with_expiry(Some(now + 3600));
        assert!(!fresh.is_expired());
        let ttl = fresh.ttl().unwrap();
        assert!(ttl.as_secs() > 3500 && ttl.as_secs() <= 3600, "{ttl:?}");
        assert!(fresh.expires_at().is_some());

        let old = cookie_with_expiry(Some(now - 10));
        assert!(old.is_expired());
        assert_eq!(old.ttl(), Some(Duration::ZERO));

        let session = cookie_with_expiry(None);
        assert!(!session.is_expired());
        assert_eq!(session.ttl(), None);
        assert_eq!(session.expires_at(), None);
    }

    /// Build a cookie with only the expiry varying.
    fn cookie_with_expiry(expires: Option<i64>) -> Cookie {
        Cookie {
            name: "n".into(),
            value: "v".into(),
            domain: Some(".example.com".into()),
            path: "/".into(),
            secure: false,
            expires,
        }
    }

    /// A host-only cookie stays on the base host; a domain cookie is scoped to
    /// the registrable domain, so it also reaches sibling subdomains.
    #[test]
    fn replay_respects_host_only_scope() {
        let now = 1_700_000_000;
        let set = CookieSet {
            cookies: vec![
                Cookie {
                    name: "host".into(),
                    value: "v".into(),
                    domain: None,
                    path: "/".into(),
                    secure: true,
                    expires: None,
                },
                Cookie {
                    name: "domain".into(),
                    value: "v".into(),
                    domain: Some(".example.com".into()),
                    path: "/".into(),
                    secure: true,
                    expires: None,
                },
            ],
            missing: vec![],
        };

        let replays = set.replay_at("https://www.example.com", now);

        assert_eq!(replays.len(), 2);
        assert_eq!(replays[0].url, "https://www.example.com/");
        assert!(!replays[0].header.contains("Domain="), "{replays:?}");
        assert_eq!(replays[1].url, "https://example.com/");
        assert!(replays[1].header.contains("; Domain=.example.com"));
    }

    /// Expired cookies never reach the jar, and `Max-Age` is relative to `now`.
    #[test]
    fn replay_skips_expired_and_sets_max_age() {
        let now = 1_700_000_000;
        let set = CookieSet {
            cookies: vec![
                cookie_with_expiry(Some(now - 1)),
                cookie_with_expiry(Some(now + 3600)),
            ],
            missing: vec![],
        };

        let replays = set.replay_at("https://example.com", now);

        assert_eq!(replays.len(), 1, "{replays:?}");
        assert!(
            replays[0].header.ends_with("; Max-Age=3600"),
            "{}",
            replays[0].header
        );
    }

    /// The tiny URL host parser has to cope with userinfo, ports and bare hosts.
    #[test]
    fn host_of_strips_scheme_userinfo_and_path() {
        assert_eq!(host_of("https://user@host:8443/path"), "host:8443");
        assert_eq!(host_of("https://example.com"), "example.com");
        assert_eq!(host_of("example.com"), "example.com");
        assert_eq!(host_of(""), "localhost");
    }

    /// Exit code 2 becomes a typed error carrying the missing names.
    #[test]
    fn required_cookie_missing_is_a_typed_error() {
        let dir = TempDir::new().unwrap();
        let ex = extractor(&dir);
        let query = CookieQuery::file(jar(&dir))
            .domain("example.com")
            .fields(["absent"])
            .require(["absent"]);

        let err = ex.extract(&query).unwrap_err();

        assert!(
            matches!(&err, Error::RequiredMissing(names) if names == &["absent"]),
            "{err:?}"
        );
    }
}
