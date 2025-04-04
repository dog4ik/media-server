use axum::http::{HeaderMap, HeaderValue};
use reqwest::{Url, header::AUTHORIZATION};

/// Create HTTP client with provod configuration
pub fn new_client(provider: &str) -> (reqwest::Client, Url) {
    tracing::info!("Using provod agent for {} provider", provider);
    let mut base_url = Url::parse(env!("PROVOD_URL")).expect("provod url variable to be valid url");
    base_url.set_path(provider);
    let mut headers = HeaderMap::with_capacity(1);
    headers.insert(AUTHORIZATION, HeaderValue::from_static("auth_token"));
    (
        reqwest::ClientBuilder::new()
            .default_headers(headers)
            .build()
            .expect("headers are valid"),
        base_url,
    )
}
