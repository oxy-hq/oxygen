//! Customer + `org_billing` row lifecycle and org-owner lookups.

use std::collections::BTreeMap;

use chrono::Utc;
use entity::{org_billing, org_members, organizations, users};
use reqwest::Method;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::errors::BillingError;
use crate::service::BillingService;
use crate::service::stripe_shapes::StripeCustomer;

/// Tuple returned by [`BillingService::lookup_owner`]:
/// `(org_name, org_slug, owner_email)`.
pub(crate) type OwnerLookup = (String, String, String);

impl BillingService {
    /// Eager Customer create — called by `provision_subscription`. Idempotent
    /// via `customer-create:{org_id}` key + persisted lookup.
    pub async fn ensure_customer(
        &self,
        org_id: Uuid,
        owner_email: &str,
        org_name: &str,
    ) -> Result<String, BillingError> {
        let row = self.load_billing(org_id).await?;
        if let Some(cid) = row.stripe_customer_id.clone() {
            return Ok(cid);
        }

        let mut params = BTreeMap::new();
        if !owner_email.is_empty() {
            params.insert("email".into(), owner_email.to_string());
        }
        if !org_name.is_empty() {
            params.insert("name".into(), org_name.to_string());
        }
        params.insert("metadata[oxy_org_id]".into(), org_id.to_string());

        let key = format!("customer-create:{org_id}");
        let customer: StripeCustomer = self
            .client
            .form(Method::POST, "/v1/customers", &params, Some(&key))
            .await?;

        let mut am: org_billing::ActiveModel = row.into();
        am.stripe_customer_id = Set(Some(customer.id.clone()));
        am.updated_at = Set(Utc::now().into());
        am.update(&self.db).await?;
        Ok(customer.id)
    }

    /// Load the org's `org_billing` row. Rows are inserted at org creation
    /// (eager); a missing row indicates data drift and is logged loudly.
    pub async fn load_billing(&self, org_id: Uuid) -> Result<org_billing::Model, BillingError> {
        match org_billing::Entity::find()
            .filter(org_billing::Column::OrgId.eq(org_id))
            .one(&self.db)
            .await?
        {
            Some(row) => Ok(row),
            None => {
                tracing::error!(?org_id, "org_billing row missing — data drift");
                Err(BillingError::OrgBillingMissing(org_id))
            }
        }
    }

    pub async fn member_count(&self, org_id: Uuid) -> Result<i64, BillingError> {
        let n = super::seats::billable_seats(&self.db, org_id).await?;
        Ok(n as i64)
    }

    /// Strict owner lookup: `(owner_email, org_name, org_slug)`. Errors when
    /// the org or its owner isn't found — callers in admin/provisioning paths
    /// must have a recipient to send the checkout email to.
    pub async fn org_owner_and_name(
        &self,
        org_id: Uuid,
    ) -> Result<(String, String, String), BillingError> {
        match self.lookup_owner(org_id).await? {
            Some((name, slug, email)) => Ok((email, name, slug)),
            None => Err(BillingError::OrgOwnerNotFound),
        }
    }

    /// Lenient owner lookup: returns `None` instead of erroring when the org
    /// or owner is missing. Used by webhook paths where a missing recipient
    /// just means we skip the email — the org-state update still succeeds.
    pub(crate) async fn lookup_owner(
        &self,
        org_id: Uuid,
    ) -> Result<Option<OwnerLookup>, BillingError> {
        let Some(org) = organizations::Entity::find_by_id(org_id)
            .one(&self.db)
            .await?
        else {
            return Ok(None);
        };
        let Some(member) = org_members::Entity::find()
            .filter(org_members::Column::OrgId.eq(org_id))
            .filter(org_members::Column::Role.eq(org_members::OrgRole::Owner))
            .one(&self.db)
            .await?
        else {
            return Ok(None);
        };
        let Some(user) = users::Entity::find_by_id(member.user_id)
            .one(&self.db)
            .await?
        else {
            return Ok(None);
        };
        // An org owner with no email cannot be a billing contact. Unreachable
        // today — frontline users are deliberately kept out of `org_members`
        // entirely — but `None` here is a diagnosable OrgOwnerNotFound rather
        // than a Stripe customer created against an empty address.
        let Some(email) = user.email else {
            return Ok(None);
        };
        Ok(Some((org.name, org.slug, email)))
    }

    /// Owner email only — used by the admin queue listing where the org+slug
    /// are already in scope.
    pub(crate) async fn find_owner_email(&self, org_id: Uuid) -> Result<String, BillingError> {
        match self.lookup_owner(org_id).await? {
            Some((_, _, email)) => Ok(email),
            None => Err(BillingError::OrgOwnerNotFound),
        }
    }
}
