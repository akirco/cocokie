//! What to fetch: source, domain filter, field names and required names.

use crate::browser::BrowserSpec;
use std::path::PathBuf;

/// Where the cookies come from.
#[derive(Debug, Clone)]
pub enum Source {
    /// Read a browser profile through yt-dlp (needs `yt_dlp` importable).
    Browser(BrowserSpec),
    /// Read an existing Netscape/curl jar or a bare `Cookie:` header file.
    File(PathBuf),
}

/// Everything the embedded script needs, built with chained setters.
///
/// ```
/// use cocokie::{Browser, BrowserSpec, CookieQuery};
///
/// let query = CookieQuery::browser(BrowserSpec::new(Browser::Firefox))
///     .domain("x.com")
///     .fields(["auth_token", "ct0"])
///     .require(["auth_token"]);
/// # let _ = query;
/// ```
#[derive(Debug, Clone)]
pub struct CookieQuery {
    pub source: Source,
    /// `"any"` / `"*"` / empty disables domain filtering (empty is never produced
    /// by the builders here, but accepted for hand-built values).
    pub domain: String,
    /// Empty means "every unexpired cookie in the domain" (`--fields '*'`).
    pub fields: Vec<String>,
    /// Keep expired cookies too instead of dropping them (`--allow-expired`).
    pub allow_expired: bool,
    /// `None` lets the script apply its own default; `Some(vec![])` (the
    /// builders' default) requires nothing; `Some(names)` turns a missing name
    /// into [`crate::Error::RequiredMissing`].
    pub require: Option<Vec<String>>,
}

impl CookieQuery {
    /// Query a browser profile.
    pub fn browser(spec: BrowserSpec) -> Self {
        Self {
            source: Source::Browser(spec),
            domain: "any".into(),
            fields: vec![],
            allow_expired: false,
            require: Some(vec![]),
        }
    }

    /// Query a cookie file (Netscape jar or `name=value; ...` header).
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self {
            source: Source::File(path.into()),
            domain: "any".into(),
            fields: vec![],
            allow_expired: false,
            require: Some(vec![]),
        }
    }

    /// Restrict to this domain and its subdomains; `"any"` keeps everything.
    pub fn domain(mut self, d: impl Into<String>) -> Self {
        self.domain = d.into();
        self
    }

    /// Cookie names to fetch. Omit to fetch every unexpired cookie in the domain.
    pub fn fields<I, S>(mut self, fields: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.fields = fields.into_iter().map(Into::into).collect();
        self
    }

    /// Include cookies whose database expiry is already in the past.
    pub fn allow_expired(mut self, yes: bool) -> Self {
        self.allow_expired = yes;
        self
    }

    /// Names that must be present; a missing one fails with
    /// [`crate::Error::RequiredMissing`].
    pub fn require<I, S>(mut self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.require = Some(names.into_iter().map(Into::into).collect());
        self
    }

    /// Translate this query into the embedded script's command-line arguments.
    ///
    /// `json-meta` is always requested because the library needs the metadata
    /// (domain/path/secure/expires) to replay cookies correctly.
    pub(crate) fn to_args(&self) -> Vec<std::ffi::OsString> {
        let mut a: Vec<std::ffi::OsString> = Vec::new();

        a.push("--fields".into());
        if self.fields.is_empty() {
            // The script spells "everything" as a literal `*`.
            a.push("*".into());
        } else {
            a.push(self.fields.join(",").into());
        }

        a.push("--domain".into());
        a.push(self.domain.clone().into());

        a.push("--format".into());
        a.push("json-meta".into());

        if self.allow_expired {
            a.push("--allow-expired".into());
        }

        // An empty list means "require nothing" and is simply omitted.
        if let Some(req) = &self.require
            && !req.is_empty()
        {
            a.push("--require".into());
            a.push(req.join(",").into());
        }

        match &self.source {
            Source::Browser(spec) => {
                a.push("--browser".into());
                a.push(spec.to_cli_arg().into());
            }
            Source::File(path) => {
                a.push("--file".into());
                a.push(path.clone().into_os_string());
            }
        }

        a
    }
}
