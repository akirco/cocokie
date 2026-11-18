//! Browser names and the small spec builder understood by yt-dlp.

use std::fmt;

/// A browser recognised by yt-dlp's cookie extractor.
///
/// Unknown names are preserved through [`Browser::Other`] instead of being
/// rejected, so new browsers keep working as soon as yt-dlp learns them.
#[derive(Debug, Clone, PartialEq, Eq)]
#[rustfmt::skip]
pub enum Browser {
    Chrome, Chromium, Firefox, Edge, Safari, Brave, Opera, Vivaldi, Whale,
    Other(String),
}

#[rustfmt::skip]
impl Browser {
    /// Lower-case name as written on the yt-dlp command line.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Chrome   => "chrome",
            Self::Chromium => "chromium",
            Self::Firefox  => "firefox",
            Self::Edge     => "edge",
            Self::Safari   => "safari",
            Self::Brave    => "brave",
            Self::Opera    => "opera",
            Self::Vivaldi  => "vivaldi",
            Self::Whale    => "whale",
            Self::Other(s) => s,
        }
    }

    /// Map a yt-dlp browser name; unknown names are preserved as
    /// [`Browser::Other`] so newer browsers still reach the extractor.
    pub fn from_name(name: &str) -> Self {
        match name.to_ascii_lowercase().as_str() {
            "chrome" => Self::Chrome,
            "chromium" => Self::Chromium,
            "firefox" => Self::Firefox,
            "edge" => Self::Edge,
            "safari" => Self::Safari,
            "brave" => Self::Brave,
            "opera" => Self::Opera,
            "vivaldi" => Self::Vivaldi,
            "whale" => Self::Whale,
            _ => Self::Other(name.to_string()),
        }
    }
}

/// A browser plus the optional selectors, following yt-dlp's syntax:
/// `BROWSER[+KEYRING][:PROFILE][::CONTAINER]`.
///
/// Zen is not a browser yt-dlp knows by name; pass it as a Firefox profile
/// path, e.g. `BrowserSpec::new(Browser::Firefox).profile("/path/to/zen/profile")`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[rustfmt::skip]
pub struct BrowserSpec {
    pub browser:   Browser,
    pub profile:   Option<String>,
    /// Chromium on Linux only: `basictext`, `gnomekeyring`, ...
    pub keyring:   Option<String>,
    /// Firefox only: container name.
    pub container: Option<String>,
}

#[rustfmt::skip]
impl BrowserSpec {
    /// A spec with no profile, keyring or container selected.
    pub fn new(browser: Browser) -> Self {
        Self { browser, profile: None, keyring: None, container: None }
    }

    /// Select a specific profile: a name yt-dlp understands or an absolute path.
    pub fn profile(mut self, p: impl Into<String>) -> Self   { self.profile   = Some(p.into()); self }

    /// Select the Chromium keyring backend used to decrypt cookies.
    pub fn keyring(mut self, k: impl Into<String>) -> Self   { self.keyring   = Some(k.into()); self }

    /// Select a Firefox container.
    pub fn container(mut self, c: impl Into<String>) -> Self { self.container = Some(c.into()); self }

    /// Render the spec string the embedded script parses, in yt-dlp's order.
    pub fn to_cli_arg(&self) -> String {
        let mut s = self.browser.as_str().to_string();
        if let Some(k) = &self.keyring   { s.push('+'); s.push_str(k); }
        if let Some(p) = &self.profile   { s.push(':'); s.push_str(p); }
        if let Some(c) = &self.container { s.push_str("::"); s.push_str(c); }
        s
    }

    /// Parse the same syntax [`BrowserSpec::to_cli_arg`] produces:
    /// `BROWSER[+KEYRING][:PROFILE][::CONTAINER]`.
    ///
    /// A leading single letter followed by `:` is treated as a Windows drive
    /// (`C:\Users\...`), not as browser `C` with a profile.
    ///
    /// ```
    /// use cocokie::{Browser, BrowserSpec};
    ///
    /// let spec = BrowserSpec::parse("chrome+gnomekeyring:Profile 1");
    /// assert_eq!(spec.browser, Browser::Chrome);
    /// assert_eq!(spec.keyring.as_deref(), Some("gnomekeyring"));
    /// assert_eq!(spec.profile.as_deref(), Some("Profile 1"));
    ///
    /// // Windows profile paths survive intact.
    /// let spec = BrowserSpec::parse(r"firefox:C:\Users\me\Profile");
    /// assert_eq!(spec.browser, Browser::Firefox);
    /// assert_eq!(spec.profile.as_deref(), Some(r"C:\Users\me\Profile"));
    /// ```
    pub fn parse(spec: &str) -> Self {
        // The container suffix comes last, so split on the right-most `::`.
        let (head, container) = match spec.rsplit_once("::") {
            Some((head, c)) => (head, Some(c)),
            None => (spec, None),
        };

        // `:PROFILE` follows `BROWSER+KEYRING`, so cut the profile first. Only
        // do so when the left side is not a Windows drive letter.
        let (head, profile) = match head.split_once(':') {
            Some((h, p)) if !looks_like_drive(h) => (h, Some(p)),
            _ => (head, None),
        };

        // `+` separates the browser from its keyring backend.
        let (browser, keyring) = match head.split_once('+') {
            Some((b, k)) => (b, Some(k)),
            None => (head, None),
        };

        let mut out = Self::new(Browser::from_name(browser.trim()));
        if let Some(k) = keyring.filter(|k| !k.is_empty()) {
            out = out.keyring(k);
        }
        if let Some(p) = profile.filter(|p| !p.is_empty()) {
            out = out.profile(p);
        }
        if let Some(c) = container.filter(|c| !c.is_empty()) {
            out = out.container(c);
        }
        out
    }

    /// Best-effort spec for a Zen installation.
    ///
    /// Zen is Firefox-based, but yt-dlp does not recognise the name `zen`; the
    /// Firefox extractor does accept an absolute profile path. This picks the
    /// profile whose `cookies.sqlite` was modified most recently, and falls back
    /// to a plain Firefox spec when no Zen directory or profile is found.
    ///
    /// Convenience only: if you know your profile path, pass it explicitly with
    /// [`BrowserSpec::profile`].
    pub fn zen() -> Self {
        let fallback = Self::new(Browser::Firefox);
        let Some(root) = dirs::config_dir().map(|d| d.join("zen")) else {
            return fallback;
        };

        let mut profiles: Vec<(std::time::SystemTime, std::path::PathBuf)> =
            std::fs::read_dir(root)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|entry| {
                    let dir = entry.path();
                    let db = dir.join("cookies.sqlite");
                    let mtime = std::fs::metadata(&db).ok()?.modified().ok()?;
                    Some((mtime, dir))
                })
                .collect();
        profiles.sort();

        match profiles.pop() {
            Some((_, dir)) => fallback.profile(dir.to_string_lossy()),
            None => fallback,
        }
    }
}

/// `C` → true, `chrome` → false. Used to keep Windows paths out of the parser.
fn looks_like_drive(s: &str) -> bool {
    s.len() == 1 && s.chars().all(|c| c.is_ascii_alphabetic())
}

/// `Display` is just [`BrowserSpec::to_cli_arg`], handy for log lines.
impl fmt::Display for BrowserSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_cli_arg())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parsing the rendered form must reproduce the same spec.
    #[test]
    fn cli_arg_round_trips() {
        let spec = BrowserSpec::new(Browser::Chrome)
            .keyring("gnomekeyring")
            .profile("Profile 1")
            .container("work");
        assert_eq!(BrowserSpec::parse(&spec.to_cli_arg()), spec);
    }

    /// Keys and profiles land on the right fields.
    #[test]
    fn parses_keyring_and_profile() {
        let spec = BrowserSpec::parse("chrome+gnomekeyring:Profile 1");
        assert_eq!(spec.browser, Browser::Chrome);
        assert_eq!(spec.keyring.as_deref(), Some("gnomekeyring"));
        assert_eq!(spec.profile.as_deref(), Some("Profile 1"));
    }

    /// A drive letter must not be mistaken for a browser name.
    #[test]
    fn windows_profile_path_is_not_treated_as_browser() {
        let spec = BrowserSpec::parse(r"firefox:C:\Users\neil\Profile");
        assert_eq!(spec.browser, Browser::Firefox);
        assert_eq!(spec.profile.as_deref(), Some(r"C:\Users\neil\Profile"));
    }

    /// Unknown browser names are passed through rather than rejected.
    #[test]
    fn unknown_browser_is_preserved() {
        assert_eq!(
            Browser::from_name("mybrowser"),
            Browser::Other("mybrowser".into())
        );
    }
}
