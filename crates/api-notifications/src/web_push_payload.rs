//! What a Web Push says, and the encryption that lets it say anything.
//!
//! # Ids and config, encrypted to the device
//!
//! A push that says only "You have a new notification" is one people turn off.
//! So a Web Push to a browser that gave us its subscription keys carries a
//! small payload — [`Payload`], version [`PAYLOAD_VERSION`]:
//!
//! ```text
//! { v: 1, id, app_name, kind_label, href, thread? }
//! ```
//!
//! the person's inbox row, who it is from (the app's name, or "Oxygen"), the
//! kind's label from its catalogue, where a tap lands, and the thread's hash.
//! The same class of facts a native banner carries ([`super::native_push`]) —
//! config, never what the notification *says*. The title, body and link an app
//! wrote live in the org's store, and nothing here reads them.
//!
//! It is encrypted with RFC 8291 (`aes128gcm`) to the subscription's own keys,
//! so the push service relays ciphertext it cannot read. That is why the app's
//! name may ride along here but not in an APNs or FCM payload, which Apple and
//! Google can read. The encryption is the `ece` crate's — Mozilla's
//! implementation, the one Firefox's own push stack uses — never hand-rolled.
//!
//! A row without keys (one registered before they were stored, or a client
//! that does not send them) still gets the bodiless wake, and so does a row
//! whose keys cannot be used: a wake that says less beats no wake.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use entity::device_tokens;
use serde::{Deserialize, Serialize};

use super::deliver::PushSignal;
use super::settings::summary::SUMMARY_KIND;

/// The version of [`Payload`]. A service worker that does not know it shows
/// the generic line, as it does for a bodiless push.
pub const PAYLOAD_VERSION: u8 = 1;

/// Longest app name or kind label carried, in characters. A label is already
/// at most 64 ([`super::catalog::model::MAX_LABEL_CHARS`]); an app's name has
/// no limit of its own, and a banner title cut at 120 loses nothing.
const MAX_TEXT_CHARS: usize = 120;

/// The encrypted body a push service accepts (RFC 8030 §7.2 says at least
/// 4096 bytes; Mozilla's and Google's say no more). The payload is a few
/// hundred bytes; this is the line past which a send would be refused.
pub const MAX_BODY_BYTES: usize = 4096;

/// An uncompressed P-256 point: `0x04`, X, Y.
const P256DH_LEN: usize = 65;
/// RFC 8291 §3.2: the authentication secret is 16 bytes.
const AUTH_LEN: usize = 16;

/// The decrypted payload a service worker reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Payload {
    pub v: u8,
    /// The recipient's inbox row.
    pub id: String,
    /// The app's name, or "Oxygen": the notification's title.
    pub app_name: String,
    /// The kind's label (or its name, humanized): the notification's body.
    pub kind_label: String,
    /// Where a tap lands ([`href`]).
    pub href: String,
    /// The thread's hash: the notification's tag, so a thread collapses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
}

impl Payload {
    pub fn new(signal: &PushSignal) -> Self {
        let app_name = clip(signal.app_name.trim());
        Self {
            v: PAYLOAD_VERSION,
            id: signal.notification_id.to_string(),
            app_name: if app_name.is_empty() {
                "Oxygen".to_string()
            } else {
                app_name
            },
            kind_label: clip(signal.label.trim()),
            href: href(signal),
            thread: signal.thread.clone(),
        }
    }
}

/// `s`, at most [`MAX_TEXT_CHARS`] characters, cut on a character boundary.
fn clip(s: &str) -> String {
    match s.char_indices().nth(MAX_TEXT_CHARS) {
        Some((at, _)) => format!("{}…", &s[..at]),
        None => s.to_string(),
    }
}

/// Where the notifications page is.
const NOTIFICATIONS_PATH: &str = "/notifications";

/// Where tapping the push lands: the notifications page, narrowed to the
/// thread when there is one, else to this kind of this source — so the item is
/// at the top. Built from ids and config only; the app's own link is content,
/// read from the inbox once the person is signed in.
///
/// The quiet-hours summary ([`SUMMARY_KIND`]) is about several held items and
/// is no inbox kind of its own, so it opens the page unfiltered.
pub fn href(signal: &PushSignal) -> String {
    if signal.kind == SUMMARY_KIND {
        return NOTIFICATIONS_PATH.to_string();
    }
    let source = signal
        .app_id
        .map_or_else(|| "oxygen".to_string(), |app| app.to_string());
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("source", &source);
    match &signal.thread {
        Some(thread) => query.append_pair("thread", thread),
        None => query.append_pair("kind", &signal.kind),
    };
    format!("{NOTIFICATIONS_PATH}?{}", query.finish())
}

/// A subscription's keys as `PushSubscription.toJSON().keys` hands them over:
/// base64url. What `POST /api/notifications/devices` accepts beside a `web`
/// endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebPushKeys {
    pub p256dh: String,
    pub auth: String,
}

/// A subscription's keys, decoded and checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionKeys {
    p256dh: Vec<u8>,
    auth: Vec<u8>,
}

impl SubscriptionKeys {
    /// Decode and check both keys. Padding is tolerated, the standard
    /// alphabet is not: browsers hand out base64url.
    pub fn parse(keys: &WebPushKeys) -> Result<Self, String> {
        let decode = |name: &str, value: &str| {
            URL_SAFE_NO_PAD
                .decode(value.trim().trim_end_matches('='))
                .map_err(|e| format!("`{name}` is not base64url: {e}"))
        };
        let p256dh = decode("p256dh", &keys.p256dh)?;
        if p256dh.len() != P256DH_LEN || p256dh[0] != 0x04 {
            return Err("`p256dh` is not an uncompressed P-256 point".into());
        }
        let auth = decode("auth", &keys.auth)?;
        if auth.len() != AUTH_LEN {
            return Err(format!(
                "`auth` is {} bytes, expected {AUTH_LEN}",
                auth.len()
            ));
        }
        Ok(Self { p256dh, auth })
    }

    /// The keys a `device_tokens` row stores: unpadded base64url.
    pub fn to_stored(&self) -> (String, String) {
        (
            URL_SAFE_NO_PAD.encode(&self.p256dh),
            URL_SAFE_NO_PAD.encode(&self.auth),
        )
    }

    /// The row's keys: `None` when it has none (a bodiless wake), `Some(Err)`
    /// when what it has cannot be used.
    pub fn of_row(row: &device_tokens::Model) -> Option<Result<Self, String>> {
        match (&row.web_push_p256dh, &row.web_push_auth) {
            (None, None) => None,
            (Some(p256dh), Some(auth)) => Some(Self::parse(&WebPushKeys {
                p256dh: p256dh.clone(),
                auth: auth.clone(),
            })),
            _ => Some(Err("only one of the two subscription keys is stored".into())),
        }
    }
}

/// `payload`, as the `aes128gcm` body of one push to `keys`'s subscription
/// (RFC 8291: a fresh ephemeral key and salt per message, so two sends of the
/// same payload never share a ciphertext).
pub fn encrypt(keys: &SubscriptionKeys, payload: &Payload) -> Result<Vec<u8>, String> {
    let plaintext = serde_json::to_vec(payload).map_err(|e| e.to_string())?;
    let body = ece::encrypt(&keys.p256dh, &keys.auth, &plaintext)
        .map_err(|e| format!("could not encrypt the push payload: {e}"))?;
    if body.len() > MAX_BODY_BYTES {
        return Err(format!(
            "the encrypted payload is {} bytes, over the {MAX_BODY_BYTES} a push service accepts",
            body.len()
        ));
    }
    Ok(body)
}

#[cfg(test)]
#[path = "web_push_payload_tests.rs"]
mod tests;
