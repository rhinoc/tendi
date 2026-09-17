use anyhow::{Context, Result};
use serde::Deserialize;
use std::time::Duration;

pub(super) fn get_json<T: for<'de> Deserialize<'de>>(url: &str) -> Result<T> {
    let response = ureq::get(url)
        .timeout(Duration::from_secs(12))
        .set("Accept", "application/json")
        .call()
        .with_context(|| format!("failed to fetch {url}"))?;
    let body = response
        .into_string()
        .with_context(|| format!("failed to read {url}"))?;
    serde_json::from_str(&body).with_context(|| format!("invalid JSON from {url}"))
}

pub(super) fn encode_query(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

#[cfg(test)]
#[path = "http_tests.rs"]
mod tests;
