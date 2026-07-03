use crate::config::ProviderConfig;
use reqwest::header::{HeaderMap, HeaderValue};

pub fn build_request_headers(
    _config: &ProviderConfig,
    client_auth_header: Option<&str>,
) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", HeaderValue::from_static("application/json"));
    if let Some(auth) = client_auth_header {
        if let Ok(val) = auth.parse() {
            headers.insert("Authorization", val);
        }
    }
    headers
}
