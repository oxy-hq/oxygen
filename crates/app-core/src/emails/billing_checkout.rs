//! Email sent to an org owner when admin provisions billing via a Stripe
//! Checkout Session. The link sends the customer through a hosted flow that
//! collects billing address, tax ID, and payment method in one step.
//!
//! Re-uses the magic-link SES config for the sender identity. If the
//! magic-link email config is missing (typical for `oxy serve --local`), the
//! send is reported as `Skipped` so the admin UI can offer a manual-copy
//! fallback.

use chrono::{DateTime, Utc};
use handlebars::Handlebars;
use once_cell::sync::Lazy;
use oxy_shared::errors::OxyError;

use crate::emails::{
    EmailMessage, EmailProvider, local_test::LocalTestEmailProvider, ses::SesEmailProvider,
};

static CHECKOUT_TEMPLATE: Lazy<Handlebars<'static>> = Lazy::new(|| {
    let mut hbs = Handlebars::new();
    hbs.register_template_string("billing_checkout", include_str!("billing_checkout.hbs"))
        .expect("billing_checkout.hbs is valid");
    hbs
});

pub struct CheckoutEmail<'a> {
    pub to_email: &'a str,
    pub org_name: &'a str,
    pub checkout_url: &'a str,
    pub expires_at: DateTime<Utc>,
}

pub enum EmailSendOutcome {
    Sent,
    Skipped { reason: String },
}

pub async fn send_checkout_email(args: CheckoutEmail<'_>) -> Result<EmailSendOutcome, OxyError> {
    let Some(config) = oxy::config::oxy::get_oxy_config()
        .ok()
        .and_then(|c| c.authentication)
        .and_then(|a| a.magic_link)
    else {
        let reason = "magic-link email config is not set; checkout link not delivered".to_string();
        tracing::warn!("{reason}");
        return Ok(EmailSendOutcome::Skipped { reason });
    };

    let expires_human = args
        .expires_at
        .format("%B %-d, %Y at %H:%M UTC")
        .to_string();
    let subject = format!("Complete your subscription for {}", args.org_name);
    let text_body = format!(
        "To finish setting up {org}, please open the link below and provide your billing address, tax ID, and payment method.\n\nThe link expires on {expires}.\n\nComplete checkout:\n{url}\n",
        org = args.org_name,
        expires = expires_human,
        url = args.checkout_url,
    );

    let message = EmailMessage {
        subject,
        html_body: build_html(args.org_name, args.checkout_url, &expires_human)?,
        text_body,
    };

    if std::env::var("MAGIC_LINK_LOCAL_TEST").is_ok() {
        LocalTestEmailProvider
            .send(&config.from_email, args.to_email, message)
            .await?;
    } else {
        SesEmailProvider::new(config.aws_region.as_deref())
            .await
            .send(&config.from_email, args.to_email, message)
            .await?;
    }
    Ok(EmailSendOutcome::Sent)
}

fn build_html(org_name: &str, checkout_url: &str, expires_human: &str) -> Result<String, OxyError> {
    let data = serde_json::json!({
        "org_name": org_name,
        "checkout_url": checkout_url,
        "expires_at_human": expires_human,
        "year": Utc::now().format("%Y").to_string(),
    });
    CHECKOUT_TEMPLATE
        .render("billing_checkout", &data)
        .map_err(|e| OxyError::RuntimeError(format!("Failed to render checkout template: {e}")))
}
