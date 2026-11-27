//! The weekly custom-app usage report, as mail: how it leaves.
//!
//! Delivery only. What goes in the report, and what the mail looks like (the
//! template, its view and `render`), are `oxy_usage_report::email`'s, which
//! hands the caller an [`EmailMessage`] ready to send.
//!
//! Same sender as the other platform mail (the magic-link SES identity). Two
//! mailers, because two things send it:
//!
//! - the weekly pass sends **real email only**. Where mail is previewed in the
//!   browser (a dev box, CI), a scheduled report would open a tab on whoever's
//!   machine runs the server, every week, unasked;
//! - a copy somebody asks for from the console is sent, or previewed where
//!   that is how the deployment shows mail.

use std::sync::Arc;

use oxy_shared::errors::OxyError;
use serde::Serialize;

use crate::emails::{
    EmailMessage, EmailProvider, local_test::LocalTestEmailProvider, ses::SesEmailProvider,
    token_mail,
};

/// How this deployment gets the report to a person.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryMode {
    /// Sent through SES.
    Email,
    /// Rendered to a file and opened in the browser; nothing is sent.
    Preview,
    /// No sender is configured.
    Off,
}

pub fn delivery_mode() -> DeliveryMode {
    mode_for(sender().is_some(), token_mail::previews_locally())
}

fn mode_for(sender_configured: bool, previews_locally: bool) -> DeliveryMode {
    match (previews_locally, sender_configured) {
        (true, _) => DeliveryMode::Preview,
        (false, true) => DeliveryMode::Email,
        (false, false) => DeliveryMode::Off,
    }
}

fn sender() -> Option<oxy::config::auth::MagicLinkAuth> {
    oxy::config::oxy::get_oxy_config()
        .ok()
        .and_then(|c| c.authentication)
        .and_then(|a| a.magic_link)
}

pub struct ReportMailer {
    provider: Arc<dyn EmailProvider>,
    from: String,
    mode: DeliveryMode,
}

impl ReportMailer {
    /// For the weekly pass: real email, or nothing. See the module docs.
    pub async fn for_schedule() -> Option<Self> {
        match delivery_mode() {
            DeliveryMode::Email => Self::ses().await,
            DeliveryMode::Preview | DeliveryMode::Off => None,
        }
    }

    /// For a copy somebody asked for: real email, or a preview.
    pub async fn for_request() -> Option<Self> {
        match delivery_mode() {
            DeliveryMode::Email => Self::ses().await,
            DeliveryMode::Preview => Some(Self::with_provider(
                Arc::new(LocalTestEmailProvider),
                DeliveryMode::Preview,
            )),
            DeliveryMode::Off => None,
        }
    }

    async fn ses() -> Option<Self> {
        let config = sender()?;
        Some(Self {
            provider: Arc::new(SesEmailProvider::new(config.aws_region.as_deref()).await),
            from: config.from_email,
            mode: DeliveryMode::Email,
        })
    }

    /// A mailer over any provider — what the tests send through.
    pub fn with_provider(provider: Arc<dyn EmailProvider>, mode: DeliveryMode) -> Self {
        Self {
            provider,
            from: sender().map(|c| c.from_email).unwrap_or_default(),
            mode,
        }
    }

    pub fn mode(&self) -> DeliveryMode {
        self.mode
    }

    pub async fn send(&self, to: &str, message: EmailMessage) -> Result<(), OxyError> {
        self.provider.send(&self.from, to, message).await
    }
}

#[cfg(test)]
#[path = "usage_report_tests.rs"]
mod tests;
