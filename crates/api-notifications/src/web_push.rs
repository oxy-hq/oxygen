//! A real Web Push sender, behind the [`Push`](super::deliver::Push) seam.
//!
//! # Why Web Push first
//!
//! APNs and FCM both need a vendor account before a single line can be tested —
//! an Apple developer membership, a Firebase project. Web Push needs a keypair
//! you generate yourself, and it covers Android Chrome and every desktop
//! browser, which is the whole audience an installed PWA has. The native apps'
//! transports ([`super::apns`], [`super::fcm`]) sit beside it, and
//! [`super::push_routing`] hands each one only its own platform's rows.
//!
//! # What the message carries: ids and config, encrypted to the device
//!
//! The first version sent a bodiless push (permitted by RFC 8030), and the
//! service worker showed "You have a new notification" for every one. That
//! kept the push service from seeing anything and kept crypto out of this
//! adapter — and it made a push nobody could act on, so a push people turn
//! off.
//!
//! A browser that registered its subscription keys now gets a payload
//! ([`super::web_push_payload`]): the inbox row's id, the app's name (or
//! "Oxygen"), the kind's label, where a tap lands, and the thread's hash.
//! Still ids and config, never the title, body or link an app wrote, which
//! stay in the org's store behind sign-in. What changed is what the push
//! service could see, and it did not grow: the payload is encrypted to the
//! subscription (RFC 8291 `aes128gcm`, the `ece` crate), so the service relays
//! ciphertext and learns what it learned before — that a device got a push,
//! roughly how big, and when. That is also why the app's name rides here and
//! not in a native banner ([`super::native_push`]): APNs and FCM payloads are
//! readable by Apple and Google.
//!
//! The notification is still a **wake signal, not the message**, the same
//! shape chat's `LISTEN/NOTIFY` uses: the durable row written by
//! [`send`](super::send::send) is the source of truth, a client that misses a
//! wake is repaired by the next one or by a navigation, and the open tab is
//! told to re-read `/api/notifications`. A row with no keys — registered
//! before they were stored — or with keys that cannot be used still gets the
//! bodiless wake and today's generic line.
//!
//! VAPID is unchanged: an ES256 JWT, which `jsonwebtoken` signs.
//!
//! # Configuration
//!
//! Three env vars, and all three must be present or the adapter is not
//! registered at all — a half-configured sender that silently drops everything
//! is precisely what [`Push::name`](super::deliver::Push::name) exists to make
//! impossible.
//!
//! * `OXY_VAPID_PRIVATE_KEY` — the EC private key, PEM (PKCS#8); `\n`
//!   escapes are read as newlines, as APNs' key is.
//! * `OXY_VAPID_PUBLIC_KEY`  — the uncompressed P-256 point, base64url, no pad.
//!   This is what the browser passes to `pushManager.subscribe`, so it has to
//!   match the private key or every send is rejected.
//! * `OXY_VAPID_SUBJECT`     — `mailto:` or `https:`, per RFC 8292. Push
//!   services use it to reach an operator when a sender misbehaves.
//!
//! Generating the pair is an operator runbook in
//! `internal-docs/custom-app-notifications.md` → Rollout.

use std::net::{IpAddr, Ipv4Addr};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use entity::device_tokens;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use oxy::database::client::establish_connection;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Serialize;
use tracing::{info, warn};

use super::deliver::{Push, PushReport, PushSignal};
use super::native_push::unescape_pem;
use super::web_push_payload::{Payload, SubscriptionKeys, encrypt};

/// The `web` platform value in `device_tokens`. APNs and FCM rows go to their
/// own transports: an APNs token sent to a Web Push endpoint would be a POST to
/// a URL that is not one.
pub(super) const PLATFORM_WEB: &str = "web";

/// How long the push service should hold the message for a device that is
/// offline. Four hours: a wake signal that arrives a day later tells a shift
/// worker about work whose shift has ended.
const TTL_SECONDS: u32 = 4 * 60 * 60;

/// VAPID assertion lifetime. RFC 8292 caps it at 24h; short is better because
/// the assertion is bearer-ish, and there is no cost to minting one per send.
const VAPID_TTL_SECONDS: u64 = 60 * 60;

/// One outbound POST at a time per notification is fine — a user has a handful
/// of devices — but a slow push service must not hold the others up.
const MAX_CONCURRENT: usize = 8;

/// Why a Web Push configuration was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// Not all three vars are set.
    Missing,
    /// `OXY_VAPID_SUBJECT` is not a `mailto:` or `https:` URI (RFC 8292).
    Subject,
    /// `OXY_VAPID_PUBLIC_KEY` is not a base64url uncompressed P-256 point.
    PublicKey(String),
    /// `OXY_VAPID_PRIVATE_KEY` is not a usable EC PEM.
    PrivateKey(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => write!(
                f,
                "set OXY_VAPID_PRIVATE_KEY, OXY_VAPID_PUBLIC_KEY and OXY_VAPID_SUBJECT"
            ),
            Self::Subject => write!(f, "OXY_VAPID_SUBJECT must be a mailto: or https: URI"),
            Self::PublicKey(why) => write!(f, "OXY_VAPID_PUBLIC_KEY {why}"),
            Self::PrivateKey(why) => write!(f, "OXY_VAPID_PRIVATE_KEY is unusable: {why}"),
        }
    }
}

/// The blocked IPv4 ranges, identical to `validate_egress`'s. Kept as its own
/// function so the two can be compared line for line.
fn ipv4_blocked(v4: &Ipv4Addr) -> bool {
    v4.is_private()
        || v4.is_loopback()
        || v4.is_link_local()
        || v4.is_unspecified()
        || v4.is_broadcast()
}

#[derive(Serialize)]
struct VapidClaims<'a> {
    aud: &'a str,
    exp: u64,
    sub: &'a str,
}

/// Is this a VAPID application server key the browser would accept?
///
/// It is the uncompressed P-256 point the page passes to
/// `pushManager.subscribe`: 65 bytes, `0x04` then X then Y, base64url with no
/// padding. Checking the encoding and the shape catches every mistake short of
/// "valid point, wrong keypair" — which only a real send can catch.
fn check_public_key(key: &str) -> Result<(), String> {
    if key.contains('=') || key.contains('+') || key.contains('/') {
        return Err("must be base64url without padding, not standard base64".into());
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(key)
        .map_err(|e| format!("is not base64url: {e}"))?;
    if bytes.len() != 65 {
        return Err(format!(
            "decodes to {} bytes, expected 65 for an uncompressed P-256 point",
            bytes.len()
        ));
    }
    if bytes[0] != 0x04 {
        return Err(format!(
            "starts with {:#04x}, expected 0x04 for an uncompressed point",
            bytes[0]
        ));
    }
    Ok(())
}

pub struct WebPush {
    key: EncodingKey,
    public_key: String,
    subject: String,
    http: reqwest::Client,
}

impl WebPush {
    /// The application server key a browser needs to subscribe.
    ///
    /// `PushManager.subscribe` takes `applicationServerKey`, and it is the
    /// VAPID PUBLIC key — the same one every assertion this sender signs is
    /// verified against. Without it a browser cannot create a subscription at
    /// all, so a deployment can have a fully working sender and no possible
    /// subscriber, which is what this exists to fix.
    ///
    /// Public by nature: it is published to the push service on every send and
    /// is meaningless without the private half. Serving it is disclosure of
    /// nothing.
    pub fn public_key(&self) -> &str {
        &self.public_key
    }

    /// Build from the environment, or `None` if it is not fully configured.
    ///
    /// All-or-nothing on purpose: two of three vars set is a deployment mistake,
    /// and registering a sender that fails every send would report as
    /// "configured" while behaving worse than the logging default.
    pub fn from_env() -> Option<Self> {
        // A multi-line PEM usually reaches the environment with its newlines
        // escaped (a secret store, an `.env` file), as APNs' key does.
        let private = std::env::var("OXY_VAPID_PRIVATE_KEY")
            .ok()
            .map(|pem| unescape_pem(&pem));
        let public = std::env::var("OXY_VAPID_PUBLIC_KEY").ok();
        let subject = std::env::var("OXY_VAPID_SUBJECT").ok();
        match Self::configure(private.as_deref(), public.as_deref(), subject.as_deref()) {
            Ok(w) => Some(w),
            Err(why) => {
                warn!(%why, "web push not registered");
                None
            }
        }
    }

    /// The configuration decision, separated from the environment so each check
    /// is assertable by NAME.
    ///
    /// A typed reason rather than a bool is what lets the tests prove the
    /// subject and public-key checks pass WITHOUT committing a private key to
    /// the repo: reaching `PrivateKey` means everything before it was accepted.
    /// The first version embedded a real PEM as a fixture, which the secret
    /// scanner correctly rejected — a test key is still a private key.
    ///
    /// All-or-nothing on purpose: two of three set is a deployment mistake, and
    /// registering a sender that fails every send would report as "configured"
    /// while behaving worse than the logging default.
    pub fn configure(
        private: Option<&str>,
        public: Option<&str>,
        subject: Option<&str>,
    ) -> Result<Self, ConfigError> {
        let (private, public, subject) = match (private, public, subject) {
            (Some(a), Some(b), Some(c)) => (a, b, c),
            _ => return Err(ConfigError::Missing),
        };
        if !subject.starts_with("mailto:") && !subject.starts_with("https://") {
            return Err(ConfigError::Subject);
        }
        // Checked BEFORE the private key, so its acceptance is observable
        // without a valid one — see the note above.
        check_public_key(public).map_err(ConfigError::PublicKey)?;
        let key = EncodingKey::from_ec_pem(private.as_bytes())
            .map_err(|e| ConfigError::PrivateKey(e.to_string()))?;

        Ok(Self {
            key,
            public_key: public.to_string(),
            subject: subject.to_string(),
            http: oxy_shared::http::builder()
                // NO REDIRECTS. The endpoint is caller-supplied (see
                // `check_endpoint`), so following a 302 would let an allowed
                // host hand us an internal one and walk straight past the host
                // check there.
                .redirect(reqwest::redirect::Policy::none())
                // A timeout is not politeness. `notify` awaits this inline under
                // a contract that a push failure never propagates — a HANG is
                // not a failure, it never returns, so an unbounded call holds
                // the notifying request open for as long as a blackhole endpoint
                // keeps the socket. Which a caller can arrange.
                .timeout(Duration::from_secs(10))
                .connect_timeout(Duration::from_secs(5))
                .build()
                .map_err(|e| ConfigError::PrivateKey(e.to_string()))?,
        })
    }

    /// The `Authorization` header for one push service origin.
    ///
    /// Per-origin because `aud` is the endpoint's origin, and a push service
    /// rejects an assertion minted for another one.
    fn authorization(&self, endpoint: &str) -> Option<String> {
        let url = reqwest::Url::parse(endpoint).ok()?;
        // An RFC 6454 origin includes a non-default port, and this is exactly
        // the value a push service compares against. Serialising it by hand
        // dropped the port, which no production service exposes — but a
        // self-hosted or mock endpoint does, and that is the one setup in which
        // delivery could actually be tested.
        let aud = url.origin().ascii_serialization();
        let exp = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() + VAPID_TTL_SECONDS;

        let jwt = jsonwebtoken::encode(
            &Header::new(Algorithm::ES256),
            &VapidClaims {
                aud: &aud,
                exp,
                sub: &self.subject,
            },
            &self.key,
        )
        .ok()?;
        Some(format!("vapid t={jwt}, k={}", self.public_key))
    }

    /// Is this endpoint one we are willing to POST to?
    ///
    /// The endpoint is `device_tokens.token`, which
    /// `POST /api/notifications/devices` accepts as any non-empty string up to
    /// 512 bytes — so it is **attacker-chosen**. Without this, any authenticated
    /// caller (including a frontline worker, since that route is self-scoped by
    /// design) could register `platform: "web"` pointing at cloud metadata or an
    /// internal service and have the server POST to it on every notification,
    /// twenty devices at a time.
    ///
    /// The rule set is `validate_egress`'s, ported rather than imported:
    /// this crate must not depend on `agentic-automation` (a domain crate, per
    /// `backend-architecture.md`). Same known gap, stated the same way — this is
    /// an IP-LITERAL guard, so a hostname that RESOLVES to a private address is
    /// not caught. Disabling redirects removes the redirect-to-private vector;
    /// closing DNS rebinding needs a resolving connector.
    ///
    /// It lives at the SEND site deliberately. Validating at registration too
    /// would be defence in depth, but this is where the request is actually
    /// made, so this is where the check has to be for it to be load-bearing.
    fn check_endpoint(endpoint: &str) -> Result<(), String> {
        let url = reqwest::Url::parse(endpoint).map_err(|e| format!("invalid endpoint: {e}"))?;
        if url.scheme() != "https" {
            return Err(format!("endpoint scheme {:?} is not https", url.scheme()));
        }
        let host = url
            .host_str()
            .ok_or("endpoint has no host")?
            .to_ascii_lowercase();
        if host == "localhost" || host.ends_with(".localhost") || host == "metadata.google.internal"
        {
            return Err(format!("endpoint host {host:?} is not allowed"));
        }
        // `host_str` serialises an IPv6 literal with brackets; strip them so it
        // parses as an `IpAddr`.
        let literal = host.trim_start_matches('[').trim_end_matches(']');
        if let Ok(ip) = literal.parse::<IpAddr>() {
            let blocked = match ip {
                IpAddr::V4(v4) => ipv4_blocked(&v4),
                // Fold an IPv4-mapped literal down to its V4 form, so
                // `[::ffff:169.254.169.254]` cannot tunnel past the V4 ranges.
                IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                    Some(v4) => ipv4_blocked(&v4),
                    None => {
                        v6.is_loopback()
                            || v6.is_unspecified()
                            || v6.is_unique_local()
                            || v6.is_unicast_link_local()
                    }
                },
            };
            if blocked {
                return Err(format!("endpoint IP {ip} is in a blocked range"));
            }
        }
        Ok(())
    }

    /// Deliver one push. `Ok(true)` means the subscription is gone and the row
    /// should be dropped.
    async fn send_one(
        &self,
        row: &device_tokens::Model,
        signal: &PushSignal,
    ) -> Result<bool, String> {
        let endpoint = row.token.as_str();
        // `Err`, never `Ok(true)`. A refused endpoint is not the push service
        // telling us the subscription is gone — treating it as such would delete
        // the row and erase the evidence, which is the opposite of what should
        // happen when somebody registers a target they should not have.
        Self::check_endpoint(endpoint)?;

        let auth = self
            .authorization(endpoint)
            .ok_or_else(|| "could not build a VAPID assertion".to_string())?;

        let req = self
            .http
            .post(endpoint)
            .header("Authorization", auth)
            .header("TTL", TTL_SECONDS.to_string());
        let req = match body_for(row, signal) {
            Some(body) => req
                .header("Content-Encoding", "aes128gcm")
                .header("Content-Type", "application/octet-stream")
                .body(body),
            // No body, so no `Content-Encoding`: an `aes128gcm` header with an
            // empty payload is what a push service rejects as malformed.
            None => req.header("Content-Length", "0"),
        };
        let res = req.send().await.map_err(|e| e.to_string())?;

        let status = res.status();
        if status.is_success() {
            return Ok(false);
        }
        // 404/410 is the push service telling us this subscription no longer
        // exists. It is the ONLY authoritative signal that a token is dead —
        // `last_seen_at` pruning is a heuristic next to it — so acting on it is
        // what keeps the fan-out from growing a tail of endpoints that can
        // never be delivered to.
        if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::GONE {
            return Ok(true);
        }
        Err(format!("push service answered {status}"))
    }
}

/// The encrypted payload for `row`, or `None` for a bodiless wake: a row with
/// no keys, or keys that cannot be used. Never an error — a wake that says
/// less still wakes — but never silent either.
fn body_for(row: &device_tokens::Model, signal: &PushSignal) -> Option<Vec<u8>> {
    let encrypted =
        SubscriptionKeys::of_row(row)?.and_then(|keys| encrypt(&keys, &Payload::new(signal)));
    match encrypted {
        Ok(body) => Some(body),
        Err(why) => {
            warn!(user = %signal.user_id, error = %why, "web push sent without a payload");
            None
        }
    }
}

#[async_trait::async_trait]
impl Push for WebPush {
    async fn send(&self, tokens: &[device_tokens::Model], signal: &PushSignal) -> PushReport {
        let web: Vec<&device_tokens::Model> = tokens
            .iter()
            .filter(|t| t.platform == PLATFORM_WEB)
            .collect();
        if web.is_empty() {
            return PushReport::default();
        }

        let mut expired: Vec<uuid::Uuid> = Vec::new();
        let mut delivered = 0usize;
        let mut failed = 0usize;

        for chunk in web.chunks(MAX_CONCURRENT) {
            let results = futures::future::join_all(
                chunk
                    .iter()
                    .map(|t| async move { (t.id, self.send_one(t, signal).await) }),
            )
            .await;
            for (id, r) in results {
                match r {
                    Ok(true) => expired.push(id),
                    Ok(false) => delivered += 1,
                    Err(e) => {
                        failed += 1;
                        // Best-effort, but never silent: the row is already
                        // written, so a failed push costs immediacy rather than
                        // the notification — and an operator has to be able to
                        // tell that from "nothing was sent".
                        warn!(user = %signal.user_id, error = %e, "web push failed");
                    }
                }
            }
        }

        if !expired.is_empty() {
            // Best-effort too: a failure here costs a stale row that the next
            // send will try again and drop, never the delivery that just worked.
            match establish_connection().await {
                Ok(db) => {
                    let n = expired.len();
                    if let Err(e) = device_tokens::Entity::delete_many()
                        .filter(device_tokens::Column::Id.is_in(expired))
                        .exec(&db)
                        .await
                    {
                        warn!(error = %e, "could not drop expired push subscriptions");
                    } else {
                        info!(
                            dropped = n,
                            "dropped push subscriptions the service reported gone"
                        );
                    }
                }
                Err(e) => warn!(error = %e, "no connection to drop expired push subscriptions"),
            }
        }

        info!(
            user = %signal.user_id,
            kind = %signal.kind,
            delivered,
            failed,
            expired = web.len() - delivered - failed,
            "web push fan-out complete"
        );
        PushReport { delivered, failed }
    }

    fn name(&self) -> &'static str {
        "web-push"
    }
}

/// Register every configured push transport — Web Push, APNs, FCM — behind
/// one [`RoutingPush`](super::push_routing::RoutingPush).
///
/// Kept at this path because `serve` and `worker` call it at startup. Returns
/// whether anything registered, so the caller can say so in a log line rather
/// than leaving an operator to infer it.
pub fn register_if_configured() -> bool {
    super::push_routing::register_if_configured()
}

#[cfg(test)]
mod tests {
    use super::super::native_push::tests::{row, signal};
    use super::*;

    /// A well-formed public key: 65 bytes, 0x04 then 64 of payload, base64url.
    /// Built rather than pasted, so there is nothing secret-shaped in the file.
    fn a_public_key() -> String {
        let mut point = vec![0x04u8];
        point.extend((0..64).map(|i| i as u8));
        URL_SAFE_NO_PAD.encode(point)
    }

    /// Not a key — the point is that configuration gets THIS far, which is what
    /// proves the two checks before it accepted their inputs.
    const NOT_A_KEY: &str = "-- not a key --";

    /// Reaching `PrivateKey` means the subject and public key were accepted.
    /// This is the positive baseline, and it needs no real key to be one.
    #[test]
    fn a_valid_subject_and_public_key_get_as_far_as_the_private_key() {
        // `.err()` rather than `expect_err`: `WebPush` holds a signing key and
        // should not derive `Debug`, so the Result is not printable.
        let err = WebPush::configure(
            Some(NOT_A_KEY),
            Some(&a_public_key()),
            Some("mailto:ops@example.com"),
        )
        .err()
        .expect("a bogus PEM cannot build a sender");
        assert!(
            matches!(err, ConfigError::PrivateKey(_)),
            "configuration stopped before the private key, so an earlier check \
             rejected a valid input: {err:?}"
        );
    }

    /// `https:` is the other subject RFC 8292 allows.
    #[test]
    fn an_https_subject_is_accepted_too() {
        assert!(matches!(
            WebPush::configure(
                Some(NOT_A_KEY),
                Some(&a_public_key()),
                Some("https://example.com/ops")
            )
            .err(),
            Some(ConfigError::PrivateKey(_))
        ));
    }

    /// Everything else here is valid, so this can only fail for the reason it
    /// names — which the first version of this test could not claim.
    #[test]
    fn a_bad_subject_is_refused_by_name() {
        assert_eq!(
            WebPush::configure(Some(NOT_A_KEY), Some(&a_public_key()), Some("who-am-i")).err(),
            Some(ConfigError::Subject)
        );
    }

    /// Two of three set is a deployment mistake, and a sender that fails every
    /// send would report as "configured" while behaving worse than no sender.
    #[test]
    fn partial_configuration_is_refused() {
        assert_eq!(
            WebPush::configure(None, Some(&a_public_key()), Some("mailto:a@b.c")).err(),
            Some(ConfigError::Missing)
        );
        assert_eq!(
            WebPush::configure(Some(NOT_A_KEY), None, Some("mailto:a@b.c")).err(),
            Some(ConfigError::Missing)
        );
        assert_eq!(
            WebPush::configure(Some(NOT_A_KEY), Some(&a_public_key()), None).err(),
            Some(ConfigError::Missing)
        );
    }

    /// The public key is the value an operator is most likely to get wrong —
    /// `openssl` does not hand you this format, you derive it — and getting it
    /// wrong means every send is rejected while the sender reports healthy.
    #[test]
    fn a_malformed_public_key_is_refused_by_name() {
        for bad in [
            "not base64!",
            // standard base64 rather than base64url
            "BPk+/abc=",
            // right encoding, wrong length
            "AAAA",
        ] {
            assert!(
                matches!(
                    WebPush::configure(Some(NOT_A_KEY), Some(bad), Some("mailto:a@b.c")).err(),
                    Some(ConfigError::PublicKey(_))
                ),
                "accepted a malformed public key: {bad:?}"
            );
        }
    }

    /// 65 bytes that do not start with 0x04 is a compressed point or garbage;
    /// the browser hands us the uncompressed form.
    #[test]
    fn a_public_key_without_the_uncompressed_marker_is_refused() {
        let mut bytes = URL_SAFE_NO_PAD.decode(a_public_key()).expect("fixture");
        bytes[0] = 0x03;
        assert!(check_public_key(&URL_SAFE_NO_PAD.encode(&bytes)).is_err());
        assert!(check_public_key(&a_public_key()).is_ok());
    }

    /// The endpoint is caller-supplied, so this is the security boundary.
    #[test]
    fn only_public_https_endpoints_are_accepted() {
        assert!(WebPush::check_endpoint("https://fcm.googleapis.com/fcm/send/abc").is_ok());
        assert!(
            WebPush::check_endpoint("https://updates.push.services.mozilla.com/wpush/v2/x").is_ok()
        );

        for bad in [
            // cloud metadata, the canonical target
            "https://169.254.169.254/latest/meta-data/",
            "https://metadata.google.internal/computeMetadata/v1/",
            // loopback and private ranges
            "https://127.0.0.1/x",
            "https://localhost/x",
            "https://app.localhost/x",
            "https://10.0.0.5/x",
            "https://192.168.1.1/x",
            "https://172.16.0.1/x",
            // IPv6 loopback, ULA and link-local
            "https://[::1]/x",
            "https://[fd00::1]/x",
            "https://[fe80::1]/x",
            // an IPv4-mapped literal must not tunnel past the V4 ranges
            "https://[::ffff:169.254.169.254]/x",
            // plaintext, and a non-URL
            "http://fcm.googleapis.com/fcm/send/abc",
            "not-a-url",
        ] {
            assert!(
                WebPush::check_endpoint(bad).is_err(),
                "accepted a forbidden endpoint: {bad:?}"
            );
        }
    }

    /// The guard has to be ON the send path, not merely present.
    ///
    /// This exists because a mutation run found the gap: deleting
    /// `check_endpoint(endpoint)?` from `send_one` left every other test green,
    /// since they call the guard directly. A validator nothing calls is the
    /// same as no validator.
    ///
    /// It needs no key and no network: the guard runs before the assertion is
    /// minted and before any socket is opened.
    #[tokio::test]
    async fn send_one_refuses_a_blocked_endpoint_before_making_a_request() {
        // A sender that cannot sign is still enough to prove ordering — if the
        // guard were missing, this would fail on the VAPID step or the network
        // rather than on the endpoint.
        let w = WebPush {
            key: EncodingKey::from_secret(b"unused"),
            public_key: a_public_key(),
            subject: "mailto:ops@example.com".into(),
            http: reqwest::Client::new(),
        };
        let err = w
            .send_one(
                &row("web", "https://169.254.169.254/latest/meta-data/"),
                &signal("k"),
            )
            .await
            .expect_err("the send path POSTed to cloud metadata");
        assert!(
            err.contains("blocked range"),
            "refused for the wrong reason, so the guard may not be on this path: {err}"
        );
    }

    /// A row with keys gets the encrypted payload; one without, the bodiless
    /// wake — as does one whose keys cannot be used, rather than no push.
    #[test]
    fn a_row_with_keys_gets_a_payload_and_one_without_a_bodiless_wake() {
        let (pair, auth) = ece::generate_keypair_and_auth_secret().expect("keygen");
        let bare = row("web", "https://push.example/a");
        let keyed = device_tokens::Model {
            web_push_p256dh: Some(URL_SAFE_NO_PAD.encode(pair.pub_as_raw().expect("pub"))),
            web_push_auth: Some(URL_SAFE_NO_PAD.encode(auth)),
            ..bare.clone()
        };
        let s = PushSignal {
            label: "Run failed".into(),
            ..signal("oxy.run_failed")
        };

        let body = body_for(&keyed, &s).expect("a keyed row gets a payload");
        let plaintext = ece::decrypt(&pair.raw_components().expect("components"), &auth, &body)
            .expect("the browser can read it");
        let payload: Payload = serde_json::from_slice(&plaintext).expect("a v1 payload");
        assert_eq!(payload, Payload::new(&s));

        assert!(body_for(&bare, &s).is_none(), "no keys: a bodiless wake");
        let broken = device_tokens::Model {
            web_push_auth: Some("not a key".into()),
            ..keyed
        };
        assert!(
            body_for(&broken, &s).is_none(),
            "unusable keys fall back to the wake"
        );
    }

    /// The TTL is a product decision, not a default: a wake signal delivered a
    /// day late tells a shift worker about work whose shift has ended.
    #[test]
    fn the_ttl_does_not_outlive_a_shift() {
        assert!(TTL_SECONDS <= 12 * 60 * 60);
    }
}
