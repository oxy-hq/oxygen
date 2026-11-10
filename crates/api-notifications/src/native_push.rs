//! What the native transports ([`super::apns`], [`super::fcm`]) share: the
//! words a phone shows, and the fan-out that turns per-device outcomes into a
//! [`PushReport`].
//!
//! # The banner names the kind, never the content
//!
//! A native push cannot be bodiless the way a Web Push wake is: iOS shows
//! nothing for an alert without text, and Android only draws a backgrounded
//! app's notification from a `notification` block. So it carries what the
//! transport already holds — the kind's label from its catalogue (or its
//! name, humanized), the inbox row's id, and the kind's category, level and
//! thread hash ([`PushSignal`]) — and nothing the notification *says*. The
//! phone fetches the rest from `GET /api/notifications/{id}`, over an
//! authenticated session, and the push services never see an org's words.

use std::future::Future;

use entity::device_tokens;
use oxy::database::client::establish_connection;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use tracing::{info, warn};
use uuid::Uuid;

use super::deliver::{PushReport, PushSignal};

/// The banner title. The product, not the app: Apple and Google can read a
/// native payload, so which app it came from is read from the inbox. (Web
/// Push names the app, because its payload is encrypted to the device —
/// `web_push_payload`.)
pub(super) const TITLE: &str = "Oxygen";

/// How long a push service should hold a message for an offline device —
/// Web Push's choice, for its reason: a shift worker told a day late is told
/// about work whose shift has ended.
pub(super) const TTL_SECONDS: u64 = 4 * 60 * 60;

/// Requests in flight per fan-out. A person has a handful of devices; this only
/// stops one slow device from holding the rest up.
const MAX_CONCURRENT: usize = 8;

/// What one device's send came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Outcome {
    Delivered,
    /// The push service says this token will never be delivered to again.
    /// The ONLY reason a row is deleted.
    Gone,
    /// Anything else, with why. Never deletes the row: a refused or failed
    /// send is evidence, and a misconfiguration must not erase every device.
    Failed(String),
}

/// Send to every row, at most [`MAX_CONCURRENT`] at a time. Returns the report
/// and the ids of the rows the service called gone, for [`drop_gone`].
///
/// Separate from the delete so it is testable without a database.
pub(super) async fn fan_out<'a, F, Fut>(
    transport: &'static str,
    rows: &[&'a device_tokens::Model],
    signal: &PushSignal,
    send_one: F,
) -> (PushReport, Vec<Uuid>)
where
    F: Fn(&'a str) -> Fut,
    Fut: Future<Output = Outcome>,
{
    let mut report = PushReport::default();
    let mut gone = Vec::new();
    for chunk in rows.chunks(MAX_CONCURRENT) {
        let results = futures::future::join_all(chunk.iter().map(|row| {
            let row: &'a device_tokens::Model = *row;
            let send = send_one(&row.token);
            async move { (row.id, send.await) }
        }))
        .await;
        for (id, outcome) in results {
            match outcome {
                Outcome::Delivered => report.delivered += 1,
                Outcome::Gone => gone.push(id),
                Outcome::Failed(why) => {
                    report.failed += 1;
                    // Never the token: it identifies a device install.
                    warn!(user = %signal.user_id, transport, error = %why, "push failed");
                }
            }
        }
    }
    info!(
        user = %signal.user_id,
        kind = %signal.kind,
        transport,
        delivered = report.delivered,
        failed = report.failed,
        gone = gone.len(),
        "push fan-out complete"
    );
    (report, gone)
}

/// Every row's outcome is `failed` for the same reason — no credential to send
/// with. Counted per device, as a real send would be.
pub(super) fn all_failed(
    transport: &'static str,
    rows: usize,
    signal: &PushSignal,
    why: &str,
) -> PushReport {
    warn!(user = %signal.user_id, transport, devices = rows, error = %why, "push not attempted");
    PushReport {
        delivered: 0,
        failed: rows,
    }
}

/// Delete the rows a push service called gone. Best-effort, as Web Push's is:
/// a failure costs a stale row the next send tries again, never the delivery
/// that worked.
pub(super) async fn drop_gone(transport: &'static str, ids: Vec<Uuid>) {
    if ids.is_empty() {
        return;
    }
    let n = ids.len();
    match establish_connection().await {
        Ok(db) => match device_tokens::Entity::delete_many()
            .filter(device_tokens::Column::Id.is_in(ids))
            .exec(&db)
            .await
        {
            Ok(_) => info!(
                transport,
                dropped = n,
                "dropped device tokens the service reported gone"
            ),
            Err(e) => warn!(transport, error = %e, "could not drop gone device tokens"),
        },
        Err(e) => warn!(transport, error = %e, "no connection to drop gone device tokens"),
    }
}

/// The rows for one platform, in order.
pub(super) fn rows_for<'a>(
    tokens: &'a [device_tokens::Model],
    platform: &str,
) -> Vec<&'a device_tokens::Model> {
    tokens.iter().filter(|t| t.platform == platform).collect()
}

/// Read a PEM from an env value that may carry its newlines escaped (`\n`),
/// which is how a multi-line secret survives most secret stores and `.env`
/// files.
pub(super) fn unescape_pem(value: &str) -> String {
    if value.contains('\n') {
        value.to_string()
    } else {
        value.replace("\\n", "\n")
    }
}

pub(super) fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::catalog::model::humanize;

    pub fn row(platform: &str, token: &str) -> device_tokens::Model {
        let now = chrono::Utc::now().fixed_offset();
        device_tokens::Model {
            id: Uuid::new_v4(),
            user_id: Uuid::nil(),
            platform: platform.into(),
            token: token.into(),
            device_name: None,
            created_at: now,
            last_seen_at: now,
            enabled: true,
            min_level: None,
            os_permission: None,
            os_disabled: None,
            app_version: None,
            web_push_p256dh: None,
            web_push_auth: None,
        }
    }

    /// A PKCS#8 PEM around `der`, as Apple's `.p8` and Google's JSON carry it.
    fn pem(der: &[u8]) -> String {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD.encode(der);
        let lines: Vec<&str> = b64
            .as_bytes()
            .chunks(64)
            .map(|c| std::str::from_utf8(c).expect("base64 is ascii"))
            .collect();
        format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
            lines.join("\n")
        )
    }

    /// A throwaway P-256 key minted per run (nothing committed): its PEM and
    /// its public point, to verify what it signs.
    pub fn ec_key() -> (String, Vec<u8>) {
        use aws_lc_rs::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
        let pair = EcdsaKeyPair::generate(&ECDSA_P256_SHA256_FIXED_SIGNING).expect("ec keygen");
        let der = pair.to_pkcs8v1().expect("pkcs8");
        (pem(der.as_ref()), pair.public_key().as_ref().to_vec())
    }

    /// A throwaway RSA-2048 key: its PKCS#8 PEM and PKCS#1 public DER.
    pub fn rsa_key() -> (String, Vec<u8>) {
        use aws_lc_rs::encoding::AsDer;
        use aws_lc_rs::rsa::{KeyPair as RsaKeyPair, KeySize};
        use aws_lc_rs::signature::KeyPair;
        let pair = RsaKeyPair::generate(KeySize::Rsa2048).expect("rsa keygen");
        let der = pair.as_der().expect("pkcs8");
        (pem(der.as_ref()), pair.public_key().as_ref().to_vec())
    }

    /// An Oxygen-less, catalogue-less signal: an app's kind in `general`,
    /// active, no thread — what an undescribed kind resolves to.
    pub fn signal(kind: &str) -> PushSignal {
        PushSignal {
            user_id: Uuid::new_v4(),
            kind: kind.into(),
            notification_id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            app_id: None,
            label: humanize(kind),
            category: "general".into(),
            level: crate::catalog::model::Level::Active,
            thread: None,
            app_name: "Oxygen".into(),
        }
    }

    #[test]
    fn humanize_turns_a_kind_into_words() {
        assert_eq!(humanize("list_overdue"), "List overdue");
        assert_eq!(humanize("delivery.received"), "Delivery received");
        assert_eq!(humanize("shift-swap_requested"), "Shift swap requested");
        // The title already says Oxygen.
        assert_eq!(humanize("oxy.run_failed"), "Run failed");
        assert_eq!(humanize("a__b"), "A b");
        assert_eq!(humanize("x"), "X");
        assert_eq!(humanize("_._"), "New notification");
    }

    #[tokio::test]
    async fn fan_out_counts_each_outcome_and_returns_the_gone() {
        let rows = [row("apns", "ok"), row("apns", "gone"), row("apns", "bad")];
        let refs: Vec<&device_tokens::Model> = rows.iter().collect();
        let (report, gone) = fan_out("test", &refs, &signal("k"), |token| async move {
            match token {
                "ok" => Outcome::Delivered,
                "gone" => Outcome::Gone,
                _ => Outcome::Failed("nope".into()),
            }
        })
        .await;
        assert_eq!(
            report,
            PushReport {
                delivered: 1,
                failed: 1
            }
        );
        assert_eq!(gone, vec![rows[1].id], "only the gone row is to be dropped");
    }

    #[test]
    fn an_escaped_pem_is_unescaped_and_a_real_one_left_alone() {
        assert_eq!(unescape_pem("a\\nb"), "a\nb");
        assert_eq!(unescape_pem("a\nb\\nc"), "a\nb\\nc");
    }
}
