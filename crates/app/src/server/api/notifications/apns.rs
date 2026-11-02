//! Apple Push Notification service, token-based (`.p8`) auth, behind the
//! [`Push`] seam.
//!
//! One HTTP/2 `POST /3/device/<token>` per device, authorized by an ES256
//! provider token Oxy signs itself. The alert names the kind and the inbox
//! row's id, never what the notification says ([`super::native_push`]).
//!
//! # Configuration
//!
//! All-or-nothing on the three credentials, as Web Push is: a half-configured
//! sender that fails every send would report as configured.
//!
//! * `OXY_APNS_KEY_P8`   — the `.p8` file's contents (PKCS#8 PEM; `\n` escapes ok).
//! * `OXY_APNS_KEY_ID`   — the key's 10-character id (the JWT `kid`).
//! * `OXY_APNS_TEAM_ID`  — the 10-character Apple team id (the JWT `iss`).
//! * `OXY_APNS_TOPIC`    — the app's bundle id; default `com.oxygenhq.app`.
//! * `OXY_APNS_SANDBOX`  — `true` for development builds' tokens.
//!
//! A sandbox token sent to the production host (or the reverse) is answered
//! `BadDeviceToken`. That fails the send and keeps the row, so a wrong
//! `OXY_APNS_SANDBOX` silences iOS pushes (every send fails) but deregisters
//! nobody.

use std::sync::Mutex;
use std::time::Duration;

use entity::device_tokens;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::warn;

use super::catalog::model::Level;
use super::deliver::{Push, PushReport, PushSignal};
use super::native_push::{self, Outcome, TITLE, TTL_SECONDS};

/// The `apns` platform value in `device_tokens`.
pub(super) const PLATFORM_APNS: &str = "apns";
const TRANSPORT: &str = "apns";
const PRODUCTION_HOST: &str = "https://api.push.apple.com";
const SANDBOX_HOST: &str = "https://api.sandbox.push.apple.com";
pub(super) const DEFAULT_TOPIC: &str = "com.oxygenhq.app";

/// Provider-token lifetime before it is re-signed. Apple rejects one older than
/// an hour (`ExpiredProviderToken`) and throttles one re-signed more often than
/// every 20 minutes (`TooManyProviderTokenUpdates`), so: well inside both.
const JWT_REFRESH_SECS: u64 = 50 * 60;

/// Why an APNs configuration was refused. Never carries key material.
#[derive(Debug, PartialEq, Eq)]
pub enum ConfigError {
    Missing,
    KeyId,
    TeamId,
    PrivateKey(String),
    Client(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => write!(
                f,
                "set OXY_APNS_KEY_P8, OXY_APNS_KEY_ID and OXY_APNS_TEAM_ID"
            ),
            Self::KeyId => write!(f, "OXY_APNS_KEY_ID must be 10 letters or digits"),
            Self::TeamId => write!(f, "OXY_APNS_TEAM_ID must be 10 letters or digits"),
            Self::PrivateKey(why) => write!(f, "OXY_APNS_KEY_P8 is unusable: {why}"),
            Self::Client(why) => write!(f, "could not build the HTTP/2 client: {why}"),
        }
    }
}

/// The environment, read once, so configuration is testable without it.
#[derive(Debug, Default, Clone)]
pub struct ApnsEnv {
    pub key_p8: Option<String>,
    pub key_id: Option<String>,
    pub team_id: Option<String>,
    pub topic: Option<String>,
    pub sandbox: bool,
}

impl ApnsEnv {
    pub fn from_env() -> Self {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        Self {
            key_p8: var("OXY_APNS_KEY_P8"),
            key_id: var("OXY_APNS_KEY_ID"),
            team_id: var("OXY_APNS_TEAM_ID"),
            topic: var("OXY_APNS_TOPIC"),
            sandbox: var("OXY_APNS_SANDBOX").is_some_and(|v| {
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            }),
        }
    }
}

/// Apple's key and team ids are both 10 ASCII letters or digits. A wrong one is
/// `InvalidProviderToken` on every send, so it is refused at boot instead.
fn apple_id(value: &str) -> bool {
    value.len() == 10 && value.bytes().all(|b| b.is_ascii_alphanumeric())
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct ProviderClaims {
    pub iss: String,
    pub iat: u64,
}

struct ProviderToken {
    jwt: String,
    issued_at: u64,
}

pub struct Apns {
    key: EncodingKey,
    key_id: String,
    team_id: String,
    topic: String,
    host: &'static str,
    http: reqwest::Client,
    bearer: Mutex<Option<ProviderToken>>,
}

impl Apns {
    /// Build from the environment, or `None` (with one warning) if it is not
    /// fully configured.
    pub fn from_env() -> Option<Self> {
        match Self::configure(ApnsEnv::from_env()) {
            Ok(a) => Some(a),
            Err(why) => {
                warn!(%why, "apns push not registered");
                None
            }
        }
    }

    pub fn configure(env: ApnsEnv) -> Result<Self, ConfigError> {
        let (Some(pem), Some(key_id), Some(team_id)) = (env.key_p8, env.key_id, env.team_id) else {
            return Err(ConfigError::Missing);
        };
        let (key_id, team_id) = (key_id.trim().to_string(), team_id.trim().to_string());
        if !apple_id(&key_id) {
            return Err(ConfigError::KeyId);
        }
        if !apple_id(&team_id) {
            return Err(ConfigError::TeamId);
        }
        let key = EncodingKey::from_ec_pem(native_push::unescape_pem(&pem).as_bytes())
            .map_err(|e| ConfigError::PrivateKey(e.to_string()))?;
        let http = reqwest::Client::builder()
            // APNs speaks only HTTP/2; this also pins TLS ALPN to `h2`.
            .http2_prior_knowledge()
            .redirect(reqwest::redirect::Policy::none())
            // `notify` awaits delivery under a contract that it never hangs.
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| ConfigError::Client(e.to_string()))?;
        Ok(Self {
            key,
            key_id,
            team_id,
            topic: env
                .topic
                .map(|t| t.trim().to_string())
                .unwrap_or_else(|| DEFAULT_TOPIC.to_string()),
            host: if env.sandbox {
                SANDBOX_HOST
            } else {
                PRODUCTION_HOST
            },
            http,
            bearer: Mutex::new(None),
        })
    }

    /// The provider token, re-signed once it is [`JWT_REFRESH_SECS`] old.
    fn bearer(&self, now: u64) -> Result<String, String> {
        let mut cached = self.bearer.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(t) = cached
            .as_ref()
            .filter(|t| now.saturating_sub(t.issued_at) < JWT_REFRESH_SECS)
        {
            return Ok(t.jwt.clone());
        }
        let jwt = provider_token(&self.key, &self.key_id, &self.team_id, now)?;
        *cached = Some(ProviderToken {
            jwt: jwt.clone(),
            issued_at: now,
        });
        Ok(jwt)
    }

    /// Apple said the token expired (a clock skew, a suspended process): sign a
    /// fresh one on the next send rather than failing for the rest of the hour.
    fn forget_bearer(&self) {
        *self.bearer.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }

    async fn send_one(&self, token: &str, signal: &PushSignal, bearer: &str, now: u64) -> Outcome {
        // The token is caller-registered and goes into the URL path. Apple's
        // are hex; anything else is refused, never deleted (as Web Push does).
        if token.is_empty() || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Outcome::Failed("not a hex APNs device token".into());
        }
        let mut req = self.http.post(format!("{}/3/device/{token}", self.host));
        for (name, value) in headers(&self.topic, bearer, signal, now) {
            req = req.header(name, value);
        }
        let res = match req.json(&payload(signal)).send().await {
            Ok(r) => r,
            // `without_url`: the URL carries the device token, which is never logged.
            Err(e) => return Outcome::Failed(e.without_url().to_string()),
        };
        let status = res.status().as_u16();
        let reason = reason_of(&res.text().await.unwrap_or_default());
        if status == 403 && reason.as_deref() == Some("ExpiredProviderToken") {
            self.forget_bearer();
        }
        classify(status, reason.as_deref())
    }
}

/// Sign an ES256 provider token: header `{alg, kid}`, claims `{iss, iat}`.
fn provider_token(
    key: &EncodingKey,
    key_id: &str,
    team_id: &str,
    iat: u64,
) -> Result<String, String> {
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some(key_id.to_string());
    // Apple documents `{alg, kid}` only.
    header.typ = None;
    let claims = ProviderClaims {
        iss: team_id.to_string(),
        iat,
    };
    jsonwebtoken::encode(&header, &claims, key)
        .map_err(|e| format!("could not sign the APNs provider token: {e}"))
}

/// The request headers. `apns-collapse-id` is the inbox row, so a re-delivery
/// of the same notification replaces its banner rather than stacking a second.
/// A passive kind goes at priority 5, which Apple delivers when the phone's
/// power allows; everything else at 10.
pub(super) fn headers(
    topic: &str,
    bearer: &str,
    signal: &PushSignal,
    now: u64,
) -> Vec<(&'static str, String)> {
    let priority = if signal.level == Level::Passive {
        "5"
    } else {
        "10"
    };
    vec![
        ("apns-topic", topic.to_string()),
        ("apns-push-type", "alert".to_string()),
        ("apns-priority", priority.to_string()),
        ("apns-expiration", (now + TTL_SECONDS).to_string()),
        ("apns-collapse-id", signal.notification_id.to_string()),
        ("authorization", format!("bearer {bearer}")),
    ]
}

/// iOS `interruption-level` for a kind's level. `critical` needs Apple's
/// critical-alerts entitlement, which the app does not hold, so a critical
/// kind is `time-sensitive`: it breaks through a Focus, not the mute switch.
pub(super) fn interruption_level(level: Level) -> &'static str {
    match level {
        Level::Passive => "passive",
        Level::Active => "active",
        Level::TimeSensitive | Level::Critical => "time-sensitive",
    }
}

/// The body. `mutable-content` lets a Notification Service Extension fetch the
/// item and rewrite the banner on the phone, where the content may be. The
/// `category` (`<app id|oxygen>.<kind>`) is the `UNNotificationCategory` that
/// registers the kind's actions from the catalogue; `thread-id` groups a
/// thread, or else the kind. A passive kind makes no sound.
pub(super) fn payload(signal: &PushSignal) -> Value {
    let mut aps = json!({
        "alert": { "title": TITLE, "body": signal.label },
        "mutable-content": 1,
        "thread-id": signal.thread_or_kind(),
        "category": signal.channel_id(),
        "interruption-level": interruption_level(signal.level),
    });
    if signal.level != Level::Passive {
        aps["sound"] = json!("default");
    }
    json!({ "aps": aps, "oxy": signal.envelope() })
}

fn reason_of(body: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct ApnsError {
        reason: String,
    }
    serde_json::from_str::<ApnsError>(body)
        .ok()
        .map(|e| e.reason)
}

/// Map Apple's answer. Gone is 410, or 400 `Unregistered`: the app was removed
/// from that device. `BadDeviceToken` is a failure, never gone — Apple also
/// answers it for a token from the other environment (a sandbox build on a
/// production host), so deleting on it would deregister every device of the
/// other kind on a server both reach. `DeviceTokenNotForTopic` and every auth
/// error are configuration, which must never delete a device either.
pub(super) fn classify(status: u16, reason: Option<&str>) -> Outcome {
    if (200..300).contains(&status) {
        return Outcome::Delivered;
    }
    let token_gone = matches!(reason, Some("Unregistered"));
    if status == 410 || (status == 400 && token_gone) {
        return Outcome::Gone;
    }
    Outcome::Failed(format!("apns answered {status} {}", reason.unwrap_or("")))
}

#[async_trait::async_trait]
impl Push for Apns {
    async fn send(&self, tokens: &[device_tokens::Model], signal: &PushSignal) -> PushReport {
        let rows = native_push::rows_for(tokens, PLATFORM_APNS);
        if rows.is_empty() {
            return PushReport::default();
        }
        let now = native_push::now_secs();
        let bearer = match self.bearer(now) {
            Ok(b) => b,
            Err(why) => return native_push::all_failed(TRANSPORT, rows.len(), signal, &why),
        };
        let (report, gone) = native_push::fan_out(TRANSPORT, &rows, signal, |token| {
            self.send_one(token, signal, &bearer, now)
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
#[path = "apns_tests.rs"]
mod tests;
