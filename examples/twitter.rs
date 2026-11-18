//! Example: pull X (Twitter) cookies from a browser and call the GraphQL
//! `Viewer` operation to verify the login.
//!
//! Usage:
//!   cargo run --example twitter            # auto-detects Zen
//!   cargo run --example twitter -- firefox
//!   cargo run --example twitter -- 'chrome:Profile 1'
//!
//! X's GraphQL queryId changes with front-end releases, so it is discovered
//! from the `main.js` referenced by the web page; `X_VIEWER_QUERY_ID` can
//! override it for debugging.

mod common;

use cocokie::{CookieExtractor, CookieQuery};

/// Public bearer token embedded in X's web bundle. Not a user credential; it
/// can break when X changes its front end.
const BEARER: &str = "AAAAAAAAAAAAAAAAAAAAANRILgAAAAAAnNwIzUejRCOuH5E6I8xnZz4puTs%3D1Zv7ttfk8LF81IUq16cHjhLTvJu4FA33AGWWjCpTnA";
/// queryId observed in 2026-11, used only if online discovery fails.
const VIEWER_QUERY_ID_FALLBACK: &str = "TpQxQtWMTMnIAMPlMGMqNw";

fn main() -> anyhow::Result<()> {
    // Pick the browser from argv, falling back to the newest Zen profile.
    let spec = common::spec_from_args();
    println!("browser: {spec}");

    // No explicit fields: fetch every unexpired cookie for the domain.
    // auth_token + ct0 are the two that carry the login session.
    let query = CookieQuery::browser(spec)
        .domain("x.com")
        .require(["auth_token", "ct0"]);

    let set = CookieExtractor::new().extract(&query)?;
    let ct0 = set
        .value("ct0")
        .ok_or_else(|| anyhow::anyhow!("ct0 is missing"))?;

    // 1. Replay the cookies into reqwest's jar using their own metadata.
    let jar = common::cookie_jar(&set, "https://x.com");

    // 2. Build a client carrying those cookies (blocking; the example needs no
    //    async runtime).
    let client = reqwest::blocking::Client::builder()
        .cookie_provider(jar)
        .user_agent(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36",
        )
        .build()?;

    // 3. Discover the current Viewer queryId, then call the API.
    let query_id = viewer_query_id(&client);
    let resp: serde_json::Value = client
        .get(format!("https://x.com/i/api/graphql/{query_id}/Viewer"))
        .query(&[("variables", "{}"), ("features", "{}")])
        .header("authorization", format!("Bearer {BEARER}"))
        .header("x-csrf-token", ct0)
        .header("x-twitter-auth-type", "OAuth2Session")
        .header("x-twitter-active-user", "yes")
        .header("x-twitter-client-language", "en")
        .header("referer", "https://x.com/home")
        .send()?
        .json()?;

    // Accept both the current and the older response shapes.
    let user = &resp["data"]["viewer"]["user_results"]["result"];
    let screen_name = user["core"]["screen_name"]
        .as_str()
        .or_else(|| user["legacy"]["screen_name"].as_str());

    match screen_name {
        Some(name) => {
            let display = user["core"]["name"]
                .as_str()
                .or_else(|| user["legacy"]["name"].as_str())
                .unwrap_or(name);
            println!("logged in as: {display} (@{name})");
        }
        None => {
            let message = resp["errors"][0]["message"]
                .as_str()
                .unwrap_or("cookie is invalid, or X changed the endpoint");
            println!("not logged in, or auth_token expired: {message}");
        }
    }

    Ok(())
}

/// Use `X_VIEWER_QUERY_ID` when set, else discover it from the front end, else
/// fall back to the baked-in value.
fn viewer_query_id(client: &reqwest::blocking::Client) -> String {
    if let Ok(id) = std::env::var("X_VIEWER_QUERY_ID") {
        if !id.is_empty() {
            return id;
        }
    }
    match discover_viewer_query_id(client) {
        Some(id) => id,
        None => VIEWER_QUERY_ID_FALLBACK.to_string(),
    }
}

/// Load the logged-in home page, follow the `main.js` it references, and read
/// the queryId out of that bundle.
fn discover_viewer_query_id(client: &reqwest::blocking::Client) -> Option<String> {
    let html = client.get("https://x.com/home").send().ok()?.text().ok()?;
    let js_url = extract_main_js_url(&html)?;
    let js = client.get(js_url).send().ok()?.text().ok()?;
    extract_viewer_query_id(&js)
}

/// Find the absolute URL of the main JS bundle inside the page HTML.
fn extract_main_js_url(html: &str) -> Option<String> {
    let start = html.find("https://abs.twimg.com/responsive-web/client-web/main.")?;
    let rest = &html[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Read the `queryId` that immediately precedes `operationName:"Viewer"`.
fn extract_viewer_query_id(js: &str) -> Option<String> {
    let marker = js.find("operationName:\"Viewer\"")?;
    let before = &js[..marker];
    let start = before.rfind("queryId:\"")? + "queryId:\"".len();
    let end = before[start..].find('"')? + start;
    Some(before[start..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_query_id_before_operation_name() {
        let js = r#"x={queryId:"abc123",operationName:"Viewer",operationType:"query"}"#;
        assert_eq!(extract_viewer_query_id(js).as_deref(), Some("abc123"));
    }

    #[test]
    fn finds_main_js_url() {
        let html = r#"<script src="https://abs.twimg.com/responsive-web/client-web/main.991.js"></script>"#;
        assert_eq!(
            extract_main_js_url(html).as_deref(),
            Some("https://abs.twimg.com/responsive-web/client-web/main.991.js")
        );
    }
}
