//! Dispatcher for [`BillingNotification`]s emitted by `oxy-billing`.
//!
//! Two things per past-due episode:
//!
//! - the **inbox notice** to every owner of the org (`oxy.billing_past_due`,
//!   critical, push), written before this returns so a webhook that answered
//!   200 has recorded it. It stores the episode, not the words, and is
//!   delivered by the durable queue;
//! - the **email** to the owner `oxy-billing` looked up, spawned
//!   fire-and-forget as before. The notice does not email as well, or that
//!   owner would be told twice.
//!
//! Neither can fail the caller (a Stripe webhook or an admin resync): the
//! billing transition is already committed.

use entity::org_members::OrgRole;
use oxy::database::client::establish_connection;
use oxy_billing::service::BillingNotification;
use sea_orm::{DatabaseConnection, DbErr};
use uuid::Uuid;

use crate::emails::billing_past_due::{PastDueEmail, send_past_due_email};
use oxy_app_core::notifications::platform::{PlatformNotice, PlatformSent, notify, people};

/// Record each notification's inbox notice and spawn its email. `public_url`
/// is captured from the `BillingService`'s `StripeConfig` so it doesn't need
/// to be re-read from env on each dispatch.
pub async fn dispatch(notifications: Vec<BillingNotification>, public_url: String) {
    for notif in notifications {
        match notif {
            BillingNotification::PastDueEntered {
                org_id,
                org_name,
                org_slug,
                owner_email,
                grace_ends_at,
            } => {
                notice(org_id, grace_ends_at).await;
                let public_url = public_url.clone();
                tokio::spawn(async move {
                    let result = send_past_due_email(PastDueEmail {
                        to_email: &owner_email,
                        org_name: &org_name,
                        org_slug: &org_slug,
                        grace_ends_at,
                        public_url: &public_url,
                    })
                    .await;
                    if let Err(e) = result {
                        tracing::error!(?e, ?org_id, "past_due email send failed");
                    }
                });
            }
        }
    }
}

/// [`past_due_notice`], logged, never raised.
async fn notice(org_id: Uuid, grace_ends_at: chrono::DateTime<chrono::Utc>) {
    let result = match establish_connection().await {
        Ok(db) => past_due_notice(&db, org_id, grace_ends_at).await,
        Err(e) => Err(DbErr::Custom(e.to_string())),
    };
    match result {
        Ok(sent) => tracing::info!(?org_id, recipients = sent.recipients, "past-due notice"),
        Err(e) => tracing::error!(?org_id, error = %e, "past-due notice not recorded"),
    }
}

/// The past-due notice for the episode ending `grace_ends_at`, to every
/// active owner of `org_id`.
pub async fn past_due_notice(
    db: &DatabaseConnection,
    org_id: Uuid,
    grace_ends_at: chrono::DateTime<chrono::Utc>,
) -> Result<PlatformSent, DbErr> {
    let owners = people::org_members_from(db, org_id, OrgRole::Owner).await?;
    let notice = PlatformNotice::BillingPastDue {
        org_id,
        grace_ends_at,
    };
    notify(db, &notice, &owners).await
}
