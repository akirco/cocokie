//! Example: pull Bilibili cookies from a browser and call the "current user"
//! API to verify the login actually works.
//!
//! Usage:
//!   cargo run --example bilibili            # auto-detects Zen
//!   cargo run --example bilibili -- firefox
//!   cargo run --example bilibili -- 'chrome:Profile 1'
//!
//! Requires a network connection and a logged-in browser profile.

mod common;

use cocokie::{CookieExtractor, CookieQuery};

fn main() -> anyhow::Result<()> {
    // Pick the browser from argv, falling back to the newest Zen profile.
    let spec = common::spec_from_args();
    println!("browser: {spec}");

    let query = CookieQuery::browser(spec)
        .domain("bilibili.com")
        // No explicit fields: fetch every unexpired cookie for the domain. List
        // fields explicitly when a smaller request is preferable.
        .require(["SESSDATA"]);

    let set = CookieExtractor::new().extract(&query)?;

    // 1. Replay the cookies into reqwest's jar using their own metadata.
    let jar = common::cookie_jar(&set, "https://www.bilibili.com");

    // 2. Build a client carrying those cookies (blocking; the example needs no
    //    async runtime).
    let client = reqwest::blocking::Client::builder()
        .cookie_provider(jar)
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .build()?;

    // 3. Ask the API who is logged in.
    let resp: serde_json::Value = client
        .get("https://api.bilibili.com/x/web-interface/nav")
        .send()?
        .json()?;

    if resp["code"] == 0 {
        println!("logged in as: {}", resp["data"]["uname"]);
        println!("UID: {}", resp["data"]["mid"]);
    } else {
        println!("not logged in, or SESSDATA expired: {}", resp["message"]);
    }

    Ok(())
}
