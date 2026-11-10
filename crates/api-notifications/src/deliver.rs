//! The push-delivery seam.
//!
//! The durable inbox row is always written before delivery is attempted, and
//! never the other way round: a push that arrives for a notification the inbox
//! does not have is a tap that lands on nothing. The product's own notices go
//! through [`super::platform::notify`]; a custom app's through `ctx.notify`.

use std::sync::{Arc, OnceLock};

use entity::{device_tokens, notifications};
use serde_json::{Value, json};
use tracing::info;
use uuid::Uuid;

use super::catalog::model::Level;
use super::catalog::source::{KindInfo, Source};

/// What a push carries: ids and config, never what the notification says.
///
/// A push is a wake signal, not the message. Each transport maps what it
/// needs: APNs and FCM ([`super::apns::payload`], [`super::fcm::message`]) the
/// inbox row the phone fetches the words from, and the kind's place in its
/// catalogue — label, category, level — and the thread's hash, which are the
/// app's config, not its content. Web Push
/// ([`super::web_push_payload::Payload`]) the row, the sender's name, the
/// label and the thread, encrypted to the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushSignal {
    pub user_id: Uuid,
    pub kind: String,
    /// The recipient's `notifications` row, so a phone opens (and fetches,
    /// via `GET /api/notifications/{id}`) the item it was told about rather
    /// than "the newest unread", which two close pushes would share. An
    /// opaque id, not content. Web Push stays bodiless and ignores it.
    pub notification_id: Uuid,
    pub org_id: Uuid,
    /// The app that sent it; `None` for Oxygen's own notices.
    pub app_id: Option<Uuid>,
    /// The kind's label from its catalogue (`Temperature alarm`), or its
    /// name humanized: the banner until the phone has fetched the item.
    pub label: String,
    /// Its category path; `general` when it names none.
    pub category: String,
    pub level: Level,
    /// The thread's hash (`notifications.thread_key`), when the app gave one.
    pub thread: Option<String>,
    /// Who it is from: the app's name, or "Oxygen" for a platform notice.
    /// Config, like the label. Only Web Push carries it, because only Web Push
    /// is encrypted to the device: an APNs or FCM payload is readable by Apple
    /// or Google, so the native banner's title stays "Oxygen".
    pub app_name: String,
}

impl PushSignal {
    /// The version of [`Self::envelope`]. A client that does not know it
    /// fetches the item and shows it plainly.
    pub const ENVELOPE_VERSION: u8 = 1;

    /// The push for `signal`, a recipient's row, whose kind is `info`, sent
    /// by `app_name` (the app's, or "Oxygen").
    pub fn new(signal: &notifications::Model, info: &KindInfo, app_name: &str) -> Self {
        Self {
            user_id: signal.user_id,
            kind: signal.kind.clone(),
            notification_id: signal.id,
            org_id: signal.org_id,
            app_id: signal.app_id,
            label: info.label.clone(),
            category: info.category.clone(),
            level: info.level,
            thread: signal.thread_key.clone(),
            app_name: app_name.to_string(),
        }
    }

    pub fn source(&self) -> Source {
        match self.app_id {
            Some(_) => Source::App,
            None => Source::Oxygen,
        }
    }

    /// `oxygen`, or the app's id (two apps may share a kind name): the
    /// Android channel group, and the prefix of [`Self::channel_id`].
    pub fn group_id(&self) -> String {
        self.app_id
            .map_or_else(|| "oxygen".to_string(), |app| app.to_string())
    }

    /// `<group>.<kind>`: the iOS notification category (which registers the
    /// kind's actions) and the Android notification channel.
    pub fn channel_id(&self) -> String {
        format!("{}.{}", self.group_id(), self.kind)
    }

    /// What iOS groups by and Android replaces on: the thread, or else the
    /// kind within its source.
    pub fn thread_or_kind(&self) -> String {
        self.thread.clone().unwrap_or_else(|| self.channel_id())
    }

    /// The envelope a native payload carries (`oxy` on iOS, flattened into
    /// `data` on Android), version [`Self::ENVELOPE_VERSION`]: ids and
    /// config only.
    pub fn envelope(&self) -> Value {
        json!({
            "v": Self::ENVELOPE_VERSION,
            "id": self.notification_id.to_string(),
            "kind": self.kind,
            "org_id": self.org_id.to_string(),
            "source": self.source().as_str(),
            "app_id": self.app_id.map(|a| a.to_string()),
            "category": self.category,
            "level": self.level.as_str(),
            "thread": self.thread,
        })
    }
}

/// What one fan-out to a person's devices achieved.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PushReport {
    /// Devices the push service accepted the message for.
    pub delivered: usize,
    /// Devices whose push failed for a reason other than being gone.
    pub failed: usize,
}

/// A push transport.
///
/// One method, taking already-resolved tokens: the seam deliberately knows
/// nothing about users, orgs or the inbox, so an adapter cannot accidentally
/// become a second place that decides who gets told what.
#[async_trait::async_trait]
pub trait Push: Send + Sync {
    /// Best-effort. A failure is reported, never raised — the durable row is
    /// already written, and failing the business operation because a phone was
    /// unreachable is the wrong trade.
    ///
    /// `async` because every transport this seam exists for is network I/O:
    /// APNs is an HTTP/2 request, FCM an HTTPS call, Web Push one HTTPS POST
    /// per endpoint.
    async fn send(&self, tokens: &[device_tokens::Model], signal: &PushSignal) -> PushReport;

    /// For the log line, so an operator can tell "no push configured" from
    /// "push configured and silent".
    fn name(&self) -> &'static str;

    /// Whether this transport sends anything at all. The delivery ledger
    /// records an unconfigured one as `not_configured`, never as `sent`.
    fn configured(&self) -> bool {
        true
    }

    /// Whether `device` would reach a configured sender. A person whose every
    /// device is on an unconfigured platform is recorded `not_configured`, not
    /// `no_devices`: they do have a phone.
    fn handles(&self, _device: &device_tokens::Model) -> bool {
        self.configured()
    }
}

/// The default: records what WOULD have been sent.
///
/// Not a silent no-op. A silent one is indistinguishable from a working sender
/// whose messages nobody receives, which is the single most expensive way for
/// this subsystem to be wrong.
pub struct LoggingPush;

#[async_trait::async_trait]
impl Push for LoggingPush {
    async fn send(&self, tokens: &[device_tokens::Model], signal: &PushSignal) -> PushReport {
        info!(
            user = %signal.user_id,
            kind = %signal.kind,
            category = %signal.category,
            level = signal.level.as_str(),
            devices = tokens.len(),
            "push not configured — notification is in the inbox only"
        );
        PushReport::default()
    }
    fn name(&self) -> &'static str {
        "logging"
    }
    fn configured(&self) -> bool {
        false
    }
}

static PUSH: OnceLock<Arc<dyn Push>> = OnceLock::new();

/// Register the transport. First call wins; the app does this once at startup.
pub fn set_push(push: Arc<dyn Push>) {
    if PUSH.set(push).is_err() {
        tracing::warn!("push transport already registered; ignoring the second");
    }
}

pub(super) fn push() -> Arc<dyn Push> {
    PUSH.get().cloned().unwrap_or_else(|| Arc::new(LoggingPush))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_transport_is_loud_about_being_absent() {
        // A silent no-op is indistinguishable from a working sender whose
        // messages nobody receives. The name is what lets an operator tell
        // "push not configured" from "push configured and broken".
        assert_eq!(LoggingPush.name(), "logging");
        assert!(!LoggingPush.configured());
        assert_eq!(
            push().name(),
            "logging",
            "unregistered must fall back, not panic"
        );
    }
}
