//! Firebase Cloud Messaging, HTTP v1, behind the [`Push`] seam.
//!
//! A service account signs an RS256 assertion, Google trades it for an OAuth2
//! access token (cached until five minutes before it expires), and each device
//! gets one `messages:send`. The message names the kind and the inbox row's id,
//! never what the notification says ([`super::native_push`]).
//!
//! # Configuration
//!
//! * `OXY_FCM_SERVICE_ACCOUNT_JSON` — the service-account key file's contents.
//!   Its `project_id`, `client_email`, `private_key` and `private_key_id` are
//!   used; its `token_uri` is not — the token endpoint is fixed, so a key file
//!   cannot point the server at another host.

use std::time::Duration;

use entity::device_tokens;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::warn;

use super::catalog::model::Level;
use super::deliver::{Push, PushReport, PushSignal};
use super::native_push::{self, Outcome, TITLE, TTL_SECONDS};

/// The `fcm` platform value in `device_tokens`.
pub(super) const PLATFORM_FCM: &str = "fcm";
const TRANSPORT: &str = "fcm";
const TOKEN_URI: &str = "https://oauth2.googleapis.com/token";
const SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";
/// Google caps an assertion at an hour.
const ASSERTION_TTL_SECS: u64 = 60 * 60;
/// Refresh this long before the access token's stated expiry.
const REFRESH_MARGIN_SECS: u64 = 5 * 60;
const ENV_VAR: &str = "OXY_FCM_SERVICE_ACCOUNT_JSON";

/// Why an FCM configuration was refused. Never carries key material.
#[derive(Debug, PartialEq, Eq)]
pub enum ConfigError {
    Missing,
    Json(String),
    ProjectId,
    PrivateKey(String),
    Client(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => write!(f, "set {ENV_VAR}"),
            // serde_json's message names the field and position, not the value.
            Self::Json(why) => write!(f, "{ENV_VAR} is not a service-account key: {why}"),
            Self::ProjectId => write!(f, "{ENV_VAR} has an invalid project_id"),
            Self::PrivateKey(why) => write!(f, "{ENV_VAR} private_key is unusable: {why}"),
            Self::Client(why) => write!(f, "could not build the HTTP client: {why}"),
        }
    }
}

#[derive(Deserialize)]
struct ServiceAccount {
    #[serde(rename = "type")]
    kind: Option<String>,
    project_id: String,
    client_email: String,
    private_key: String,
    private_key_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct AssertionClaims {
    pub iss: String,
    pub scope: String,
    pub aud: String,
    pub iat: u64,
    pub exp: u64,
}

struct AccessToken {
    token: String,
    expires_at: u64,
}

pub struct Fcm {
    key: EncodingKey,
    key_id: Option<String>,
    client_email: String,
    project_id: String,
    http: reqwest::Client,
    /// `tokio`'s, held across the token fetch, so concurrent sends wait for one
    /// refresh instead of each minting their own.
    access: tokio::sync::Mutex<Option<AccessToken>>,
}

impl Fcm {
    /// Build from the environment, or `None` (with one warning) if it is not
    /// configured.
    pub fn from_env() -> Option<Self> {
        let json = std::env::var(ENV_VAR).ok().filter(|v| !v.trim().is_empty());
        match Self::configure(json.as_deref()) {
            Ok(f) => Some(f),
            Err(why) => {
                warn!(%why, "fcm push not registered");
                None
            }
        }
    }

    pub fn configure(json: Option<&str>) -> Result<Self, ConfigError> {
        let json = json.ok_or(ConfigError::Missing)?;
        let sa: ServiceAccount =
            serde_json::from_str(json).map_err(|e| ConfigError::Json(e.to_string()))?;
        if sa.kind.as_deref().is_some_and(|k| k != "service_account") {
            return Err(ConfigError::Json("`type` is not service_account".into()));
        }
        // Interpolated into the send URL: GCP ids are lowercase, digits, `-`.
        let project_ok = !sa.project_id.is_empty()
            && sa
                .project_id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !project_ok {
            return Err(ConfigError::ProjectId);
        }
        let key = EncodingKey::from_rsa_pem(native_push::unescape_pem(&sa.private_key).as_bytes())
            .map_err(|e| ConfigError::PrivateKey(e.to_string()))?;
        let http = oxy_shared::http::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| ConfigError::Client(e.to_string()))?;
        Ok(Self {
            key,
            key_id: sa.private_key_id,
            client_email: sa.client_email,
            project_id: sa.project_id,
            http,
            access: tokio::sync::Mutex::new(None),
        })
    }

    /// The signed JWT-bearer assertion Google trades for an access token.
    fn assertion(&self, now: u64) -> Result<String, String> {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = self.key_id.clone();
        let claims = AssertionClaims {
            iss: self.client_email.clone(),
            scope: SCOPE.to_string(),
            aud: TOKEN_URI.to_string(),
            iat: now,
            exp: now + ASSERTION_TTL_SECS,
        };
        jsonwebtoken::encode(&header, &claims, &self.key)
            .map_err(|e| format!("could not sign the FCM assertion: {e}"))
    }

    /// A live access token: the cached one, or a fresh one from Google.
    async fn access_token(&self) -> Result<String, String> {
        let mut cached = self.access.lock().await;
        let now = native_push::now_secs();
        if let Some(t) = cached
            .as_ref()
            .filter(|t| now + REFRESH_MARGIN_SECS < t.expires_at)
        {
            return Ok(t.token.clone());
        }
        let fresh = self.fetch_access_token(now).await?;
        let token = fresh.token.clone();
        *cached = Some(fresh);
        Ok(token)
    }

    async fn fetch_access_token(&self, now: u64) -> Result<AccessToken, String> {
        #[derive(Deserialize)]
        struct TokenResponse {
            access_token: String,
            expires_in: u64,
        }
        let assertion = self.assertion(now)?;
        let res = self
            .http
            .post(TOKEN_URI)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", assertion.as_str()),
            ])
            .send()
            .await
            .map_err(|e| format!("oauth token request failed: {e}"))?;
        let status = res.status();
        if !status.is_success() {
            // `{"error": "invalid_grant", ...}` — a code, never a credential.
            let body: Value = res.json().await.unwrap_or(Value::Null);
            let code = body.get("error").and_then(Value::as_str).unwrap_or("");
            return Err(format!("oauth token endpoint answered {status} {code}"));
        }
        let t: TokenResponse = res
            .json()
            .await
            .map_err(|e| format!("oauth token response unreadable: {e}"))?;
        Ok(AccessToken {
            token: t.access_token,
            expires_at: now + t.expires_in,
        })
    }

    async fn forget_access_token(&self) {
        *self.access.lock().await = None;
    }

    async fn send_one(&self, token: &str, signal: &PushSignal, bearer: &str) -> Outcome {
        let url = format!(
            "https://fcm.googleapis.com/v1/projects/{}/messages:send",
            self.project_id
        );
        let res = self
            .http
            .post(url)
            .bearer_auth(bearer)
            .json(&message(token, signal))
            .send()
            .await;
        let res = match res {
            Ok(r) => r,
            Err(e) => return Outcome::Failed(e.without_url().to_string()),
        };
        let status = res.status().as_u16();
        if status == 401 {
            // Revoked or expired early: mint a new one on the next send.
            self.forget_access_token().await;
        }
        classify(status, &res.text().await.unwrap_or_default())
    }
}

/// The `messages:send` body. `collapse_key` is the inbox row, so a
/// re-delivery of one notification replaces rather than stacks.
///
/// `channel_id` is `<app id|oxygen>.<kind>`: one channel per kind, under a
/// group per source, created by the app from the catalogue. Until it has
/// created one, Android falls back to the app's default channel. `tag` is the
/// thread, so a thread's newest replaces its older one in the drawer; with no
/// thread each notification stands alone. A passive kind goes at normal
/// priority; the rest high.
pub(super) fn message(token: &str, signal: &PushSignal) -> Value {
    let id = signal.notification_id.to_string();
    let (priority, notification_priority) = match signal.level {
        Level::Passive => ("normal", "PRIORITY_LOW"),
        Level::Active => ("high", "PRIORITY_DEFAULT"),
        Level::TimeSensitive | Level::Critical => ("high", "PRIORITY_HIGH"),
    };
    let mut notification = json!({
        "channel_id": signal.channel_id(),
        "notification_priority": notification_priority,
    });
    if let Some(thread) = &signal.thread {
        notification["tag"] = json!(thread);
    }
    json!({
        "message": {
            "token": token,
            "notification": { "title": TITLE, "body": signal.label },
            "data": data(signal),
            "android": {
                "priority": priority,
                "ttl": format!("{TTL_SECONDS}s"),
                "collapse_key": id,
                "notification": notification,
            },
        }
    })
}

/// The envelope as FCM `data`: every value a string, an absent one left out.
fn data(signal: &PushSignal) -> Value {
    let Value::Object(envelope) = signal.envelope() else {
        return json!({});
    };
    envelope
        .into_iter()
        .filter_map(|(k, v)| match v {
            Value::Null => None,
            Value::String(s) => Some((k, Value::String(s))),
            other => Some((k, Value::String(other.to_string()))),
        })
        .collect()
}

/// Map Google's answer. Gone is `UNREGISTERED`, or `INVALID_ARGUMENT` that
/// names the token. A bare 404 or `INVALID_ARGUMENT` about anything else is
/// not proof the device is gone — a wrong project or a malformed message would
/// otherwise delete every device on the first send.
pub(super) fn classify(status: u16, body: &str) -> Outcome {
    if (200..300).contains(&status) {
        return Outcome::Delivered;
    }
    let err: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let err = &err["error"];
    let details = err["details"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let codes: Vec<&str> = details
        .iter()
        .filter_map(|d| d["errorCode"].as_str())
        .collect();
    let status_code = err["status"].as_str().unwrap_or("");
    if codes.contains(&"UNREGISTERED") {
        return Outcome::Gone;
    }
    let invalid = status_code == "INVALID_ARGUMENT" || codes.contains(&"INVALID_ARGUMENT");
    let about_token = details.iter().any(|d| {
        d["fieldViolations"]
            .as_array()
            .is_some_and(|v| v.iter().any(|f| f["field"] == "message.token"))
    }) || err["message"]
        .as_str()
        .is_some_and(|m| m.to_ascii_lowercase().contains("registration token"));
    if invalid && about_token {
        return Outcome::Gone;
    }
    let code = codes.first().copied().unwrap_or(status_code);
    Outcome::Failed(format!("fcm answered {status} {code}"))
}

#[async_trait::async_trait]
impl Push for Fcm {
    async fn send(&self, tokens: &[device_tokens::Model], signal: &PushSignal) -> PushReport {
        let rows = native_push::rows_for(tokens, PLATFORM_FCM);
        if rows.is_empty() {
            return PushReport::default();
        }
        let bearer = match self.access_token().await {
            Ok(b) => b,
            Err(why) => return native_push::all_failed(TRANSPORT, rows.len(), signal, &why),
        };
        let (report, gone) = native_push::fan_out(TRANSPORT, &rows, signal, |token| {
            self.send_one(token, signal, &bearer)
        })
        .await;
        native_push::drop_gone(TRANSPORT, gone).await;
        report
    }

    fn name(&self) -> &'static str {
        TRANSPORT
    }
}

#[cfg(test)]
#[path = "fcm_tests.rs"]
mod tests;
