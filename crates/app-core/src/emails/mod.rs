use async_trait::async_trait;
use oxy_shared::errors::OxyError;

pub mod app_emailer;
pub mod billing_checkout;
pub mod billing_past_due;
pub mod local_test;
mod mime;
pub mod notification;
pub mod ses;
pub mod token_expiring;
pub mod token_grant_revoked;
pub mod token_leaked;
pub mod token_mail;
pub mod usage_report;

/// The kernel's, so a domain crate can write a mail without depending on the
/// crate that sends it (`oxy_usage_report::email` does). Re-exported here, at
/// the path every sender already names.
pub use oxy_shared::email::EmailMessage;

#[async_trait]
pub trait EmailProvider: Send + Sync {
    async fn send(&self, from: &str, to: &str, message: EmailMessage) -> Result<(), OxyError>;
}
