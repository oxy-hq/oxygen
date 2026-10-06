//! Seat-quantity sync: pushes member-count changes up to Stripe and runs
//! the periodic reconciler that catches drift.

use std::collections::BTreeMap;

use chrono::Utc;
use entity::{org_billing, org_members};
use reqwest::Method;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DbErr, EntityTrait,
    PaginatorTrait, QueryFilter, QuerySelect, TransactionTrait,
};
use serde_json::Value as JsonValue;
use uuid::Uuid;

use crate::errors::BillingError;
use crate::service::BillingService;

/// The org's billable seats: its `org_members` rows, and nothing else.
///
/// This is the one definition of a seat. A principal that must not be billed
/// as a person — a frontline worker, an org's **service account** — is
/// deliberately not an `org_members` row, and that is what keeps it out of
/// this count; nothing here filters it out after the fact.
pub async fn billable_seats<C: ConnectionTrait>(db: &C, org_id: Uuid) -> Result<u64, DbErr> {
    org_members::Entity::find()
        .filter(org_members::Column::OrgId.eq(org_id))
        .count(db)
        .await
}

impl BillingService {
    /// Push the current member count up to Stripe as the subscription
    /// quantity. Lock-then-read closes the rapid-member-change race: the
    /// row is exclusive-locked first, then `member_count` re-read inside the
    /// transaction so concurrent calls observe a serialized order.
    ///
    /// Proration policy depends on subscription state:
    /// - **Adding seats on `charge_automatically`** → `always_invoice`:
    ///   Stripe finalizes a separate invoice for the proration immediately
    ///   and charges the default payment method, so the org pays for the
    ///   extra seat right away instead of waiting until the next renewal.
    /// - **Removing seats**, or **adding on `send_invoice`** → `create_prorations`:
    ///   the proration is folded into the next regular invoice. Avoids
    ///   negative one-off invoices (removals) and avoids spamming the
    ///   customer with manual-pay emails for tiny prorations (send_invoice).
    pub async fn sync_seats(&self, org_id: Uuid) -> Result<(), BillingError> {
        let txn = self.db.begin().await?;

        let row = org_billing::Entity::find()
            .filter(org_billing::Column::OrgId.eq(org_id))
            .lock_exclusive()
            .one(&txn)
            .await?
            .ok_or(BillingError::OrgBillingMissing(org_id))?;

        let (Some(sub_id), Some(seat_item_id)) = (
            row.stripe_subscription_id.clone(),
            row.stripe_subscription_seat_item_id.clone(),
        ) else {
            txn.commit().await?;
            return Ok(());
        };

        let count = billable_seats(&txn, org_id).await?;

        if count as i32 == row.seats_paid {
            txn.commit().await?;
            return Ok(());
        }

        let is_adding = count as i32 > row.seats_paid;
        let sub: JsonValue = self
            .client
            .get(&format!("/v1/subscriptions/{sub_id}"))
            .await?;
        let auto_charge = sub["collection_method"].as_str() == Some("charge_automatically");
        let proration_behavior = if is_adding && auto_charge {
            "always_invoice"
        } else {
            "create_prorations"
        };

        let mut params = BTreeMap::new();
        params.insert("items[0][id]".into(), seat_item_id.clone());
        params.insert("items[0][quantity]".into(), count.to_string());
        params.insert("proration_behavior".into(), proration_behavior.into());

        // Nonce in the key prevents Stripe's 24h idempotency cache from
        // short-circuiting an oscillating count (e.g., 5→6→5) — without it,
        // the second 5-seat sync would return the cached success without
        // actually rewinding the subscription quantity.
        let key = format!("seat-sync:{}:{}:{}", org_id, seat_item_id, Uuid::new_v4());
        let _: JsonValue = self
            .client
            .form(
                Method::POST,
                &format!("/v1/subscriptions/{sub_id}"),
                &params,
                Some(&key),
            )
            .await?;

        let mut am: org_billing::ActiveModel = row.into();
        am.seats_paid = Set(count as i32);
        am.updated_at = Set(Utc::now().into());
        am.update(&txn).await?;
        txn.commit().await?;
        Ok(())
    }

    pub async fn reconcile_all_seats(&self) -> Result<(), BillingError> {
        let rows = org_billing::Entity::find()
            .filter(org_billing::Column::StripeSubscriptionSeatItemId.is_not_null())
            .all(&self.db)
            .await?;
        for row in rows {
            let count = self.member_count(row.org_id).await?;
            if count as i32 != row.seats_paid
                && let Err(e) = self.sync_seats(row.org_id).await
            {
                tracing::warn!(?e, org_id = ?row.org_id, "seat reconcile failed");
            }
        }
        Ok(())
    }
}
