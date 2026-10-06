//! `BillingService` — operational surface for the sales-gated billing flow.
//!
//! HTTP calls to Stripe go through [`StripeClient`] (reqwest-based). DB
//! access via Sea-ORM. Webhook events are parsed as `serde_json::Value`
//! since we only touch a small set of fields. See
//! `internal-docs/2026-04-28-stripe-billing.md` for the authoritative
//! design.
//!
//! The impl is split across several sibling files — each holds the
//! `impl BillingService { ... }` methods for one concern:
//!
//! | File              | Responsibility                                  |
//! |-------------------|-------------------------------------------------|
//! | `customers.rs`    | Customer + `org_billing` lifecycle, owner lookup|
//! | `webhook_apply.rs`| Webhook idempotency, event dispatch + handlers  |
//! | `portal.rs`       | Customer Portal sessions + restricted config    |
//! | `invoices.rs`     | Invoice listing, customer subscription overview |
//! | `provision.rs`    | Direct-API `/v1/subscriptions` provisioning     |
//! | `checkout.rs`     | Hosted Checkout Sessions provisioning           |
//! | `admin.rs`        | Admin queue, live detail, Price catalogue       |
//! | `seats.rs`        | Seat-quantity sync + reconcile                  |
//!
//! Pure helpers (validation, JSON extraction, mappers, formatting) live in
//! `helpers/`. Internal Stripe API response shapes live in
//! `stripe_shapes.rs`.

mod admin;
mod checkout;
mod customers;
pub(crate) mod dto;
mod helpers;
mod invoices;
mod portal;
mod provision;
mod seats;
pub use seats::billable_seats;
mod stripe_shapes;
mod webhook_apply;

pub use dto::{
    AdminOrgRow, AdminPriceDto, AdminPriceTierDto, AdminSubscriptionDetail, AdminSubscriptionItem,
    BillingNotification, CheckoutCreated, CheckoutSessionInfo, InvoiceDto, LatestInvoiceSummary,
    LookupOutcome, ProvisionCheckoutOutcome, ProvisionCheckoutRequest, ProvisionItem,
    ProvisionItemRole, ProvisionRequest, ProvisionResponse, SubscriptionOverview,
};

use sea_orm::DatabaseConnection;

use crate::client::StripeClient;
use crate::errors::BillingError;

#[derive(Clone)]
pub struct BillingService {
    pub(crate) client: StripeClient,
    pub(crate) db: DatabaseConnection,
}

impl BillingService {
    pub fn new(client: StripeClient, db: DatabaseConnection) -> Self {
        Self { client, db }
    }

    /// Public-facing app URL (used for redirect/return URLs, email links).
    /// Not a secret; safe to expose to callers.
    pub fn public_url(&self) -> &str {
        self.client.public_url()
    }

    /// Verify a Stripe webhook signature against the configured signing
    /// secret. Kept on the service so the secret never leaves this crate.
    pub fn verify_webhook_signature(
        &self,
        body: &[u8],
        header: &str,
        now_unix: i64,
    ) -> Result<(), BillingError> {
        crate::webhook::verify_signature(
            body,
            header,
            self.client.webhook_signing_secret(),
            now_unix,
        )
    }
}
