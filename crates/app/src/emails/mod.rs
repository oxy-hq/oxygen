use async_trait::async_trait;
use oxy_shared::errors::OxyError;

pub mod app_emailer;
pub mod billing_checkout;
pub mod billing_past_due;
pub mod local_test;
mod mime;
pub mod ses;
pub mod token_expiring;
pub mod token_grant_revoked;
pub mod token_leaked;
pub mod token_mail;

pub struct EmailMessage {
    pub subject: String,
    pub html_body: String,
    pub text_body: String,
}

#[async_trait]
pub trait EmailProvider: Send + Sync {
    async fn send(&self, from: &str, to: &str, message: EmailMessage) -> Result<(), OxyError>;
}
