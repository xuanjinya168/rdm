//! Construction of the shared reqwest client.

use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue, ACCEPT_ENCODING};

use crate::error::HttpError;

/// Sent as the `User-Agent` on every request.
pub const USER_AGENT: &str = concat!("RDM/", env!("CARGO_PKG_VERSION"));

/// Build a client sized for `connections` parallel segment requests.
///
/// Mirrors the Python engine's httpx setup: identity encoding (so byte ranges
/// map straight to file offsets), redirect following, and a pool a little
/// larger than the worker count. There is deliberately no total-request
/// timeout — a multi-gigabyte download must not be killed mid-stream — only a
/// connect timeout and a per-read inactivity timeout.
pub fn build_client(connections: u32) -> Result<reqwest::Client, HttpError> {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));

    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .default_headers(headers)
        .pool_max_idle_per_host(connections as usize + 4)
        .pool_idle_timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(30))
        .build()
        .map_err(HttpError::from)
}
