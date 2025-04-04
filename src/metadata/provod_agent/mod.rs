use anyhow::Context;
use axum::http::{HeaderMap, HeaderValue};
use reqwest::{Url, header::AUTHORIZATION};

use crate::config;

/// Create HTTP client with provod configuration
///
/// returns Error when url or auth token are not found
pub fn new_client(provider: &str) -> anyhow::Result<(reqwest::Client, Url)> {
    let provod_key: config::ProvodKey = config::CONFIG.get_value();
    let provod_key = provod_key.0.context("missing provod agent token")?;
    let provod_url: config::ProvodUrl = config::CONFIG.get_value();
    let provod_url = provod_url.0.context("missing provod agent url")?;
    let mut base_url = Url::parse(&provod_url).context("parse url")?;
    tracing::info!("Using provod agent for {} provider", provider);
    base_url.set_path(provider);
    let mut headers = HeaderMap::with_capacity(1);
    headers.insert(AUTHORIZATION, HeaderValue::from_str(&provod_key)?);
    Ok((
        reqwest::ClientBuilder::new()
            .default_headers(headers)
            .build()
            .expect("headers are valid"),
        base_url,
    ))
}
