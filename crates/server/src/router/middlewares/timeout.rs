use axum::extract::{FromRequestParts, Request};
use axum::http::request::Parts;
use axum::http::{HeaderName, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use std::time::Duration;

pub fn get_timeout_secs() -> u64 {
    std::env::var("OXY_REQUEST_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60)
}

/// Custom header for specifying request operation timeouts
pub const REQUEST_TIMEOUT_HEADER: &str = "X-Oxy-Request-Timeout";

/// Timeout configuration extracted from request headers or defaults
#[derive(Debug, Clone, Copy)]
pub struct TimeoutConfig {
    pub duration: Duration,
}

impl Default for TimeoutConfig {
    fn default() -> Self {
        Self {
            duration: Duration::from_secs(get_timeout_secs()),
        }
    }
}

impl TimeoutConfig {
    /// Create timeout config from seconds, clamping to max allowed value
    pub fn from_secs(seconds: u64) -> Self {
        let max_timeout = get_timeout_secs();
        let clamped_secs = seconds.min(max_timeout);
        Self {
            duration: Duration::from_secs(clamped_secs),
        }
    }

    /// Extract timeout from request headers or use default
    pub fn from_headers(headers: &axum::http::HeaderMap) -> Self {
        let header_name = HeaderName::from_bytes("x-oxy-request-timeout".as_bytes())
            .unwrap_or_else(|_| HeaderName::from_static("x-oxy-request-timeout"));

        if let Some(timeout_header) = headers.get(&header_name)
            && let Ok(timeout_str) = timeout_header.to_str()
            && let Ok(timeout_secs) = timeout_str.parse::<u64>()
        {
            return Self::from_secs(timeout_secs);
        }

        Self::default()
    }
}

/// Extract timeout config from request extensions
/// This is set by the timeout_middleware
impl<S> FromRequestParts<S> for TimeoutConfig
where
    S: Send + Sync,
{
    type Rejection = StatusCode;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        // Try to get from extensions first (set by middleware)
        if let Some(config) = parts.extensions.get::<TimeoutConfig>() {
            Ok(*config)
        } else {
            // Fallback to parsing headers directly
            Ok(TimeoutConfig::from_headers(&parts.headers))
        }
    }
}

/// Middleware to add timeout configuration to request extensions
/// This makes TimeoutConfig available to all handlers
pub async fn timeout_middleware(mut request: Request, next: Next) -> Result<Response, StatusCode> {
    let timeout_config = TimeoutConfig::from_headers(request.headers());

    // Insert timeout config into request extensions for handler access
    request.extensions_mut().insert(timeout_config);

    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_timeout_config() {
        let config = TimeoutConfig::default();
        assert_eq!(config.duration, Duration::from_secs(get_timeout_secs()));
    }

    #[test]
    fn test_timeout_config_from_secs() {
        let config = TimeoutConfig::from_secs(60);
        assert_eq!(config.duration, Duration::from_secs(60));

        // Test clamping to max value
        let config = TimeoutConfig::from_secs(500);
        assert_eq!(config.duration, Duration::from_secs(get_timeout_secs()));
    }

    #[test]
    fn test_timeout_config_from_headers() {
        use axum::http::{HeaderMap, HeaderValue};

        let mut headers = HeaderMap::new();

        // Test default when no header present
        let config = TimeoutConfig::from_headers(&headers);
        assert_eq!(config.duration, Duration::from_secs(get_timeout_secs()));

        headers.insert(REQUEST_TIMEOUT_HEADER, HeaderValue::from_static("45"));
        let config = TimeoutConfig::from_headers(&headers);
        assert_eq!(config.duration, Duration::from_secs(45));

        // Test invalid timeout header - should fall back to default
        headers.insert(REQUEST_TIMEOUT_HEADER, HeaderValue::from_static("invalid"));
        let config = TimeoutConfig::from_headers(&headers);
        assert_eq!(config.duration, Duration::from_secs(get_timeout_secs()));

        // Test timeout clamping
        headers.insert(REQUEST_TIMEOUT_HEADER, HeaderValue::from_static("600"));
        let config = TimeoutConfig::from_headers(&headers);
        assert_eq!(config.duration, Duration::from_secs(get_timeout_secs()));
    }
}
