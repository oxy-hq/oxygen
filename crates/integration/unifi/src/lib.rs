//! UniFi (Ubiquiti) cloud API client.
//!
//! Wraps the Site Manager API at `api.ui.com` plus the Protect proxy
//! under `/v1/connector/consoles/{cid}/proxy/protect/integration/v1/...`.
//!
//! Auth: `X-API-KEY` header, key generated at unifi.ui.com.

mod errors;

pub use errors::{UnifiError, UnifiResult};

use reqwest::Client;
use url::Url;

const DEFAULT_BASE_URL: &str = "https://api.ui.com";

/// HTTP client for the UniFi Site Manager + Protect cloud APIs.
#[derive(Clone, Debug)]
pub struct UnifiClient {
    base_url: Url,
    api_key: String,
    http: Client,
}

impl UnifiClient {
    /// Construct a client with the default `api.ui.com` base URL.
    pub fn new(api_key: impl Into<String>) -> UnifiResult<Self> {
        Self::with_base_url(DEFAULT_BASE_URL, api_key)
    }

    /// Construct a client with a custom base URL (testing, proxies).
    pub fn with_base_url(base_url: &str, api_key: impl Into<String>) -> UnifiResult<Self> {
        let base = Url::parse(base_url).map_err(|e| UnifiError::InvalidBaseUrl(e.to_string()))?;
        Ok(Self {
            base_url: base,
            api_key: api_key.into(),
            http: oxy_shared::http::builder()
                .user_agent("oxy-unifi/0.1")
                .build()
                .map_err(|e| UnifiError::Transport(e.to_string()))?,
        })
    }

    pub(crate) fn base_url(&self) -> &Url {
        &self.base_url
    }

    pub(crate) fn api_key(&self) -> &str {
        &self.api_key
    }

    pub(crate) fn http(&self) -> &Client {
        &self.http
    }
}

// Endpoint modules — each one defines its own request/response shapes
// and a method on UnifiClient. Kept thin: this crate is the wire layer,
// not the business logic.

pub mod devices;
pub mod hosts;
pub mod protect;
