//! Past-due notification email. Triggered when a Stripe webhook (or the
//! synchronous confirm-checkout path) reports an `active → past_due`
//! transition for an org. Sent to the org owner; informs them of the 7-day
//! grace deadline before write access is paused.
//!
//! Re-uses the magic-link SES config for the sender identity. If magic-link
//! email is not configured (typical for `oxy serve --local`), this is a
//! no-op — `oxy-billing` still records the transition; only the email is
//! suppressed.

use chrono::{DateTime, Utc};
use handlebars::Handlebars;
use once_cell::sync::Lazy;
use oxy_shared::errors::OxyError;

use crate::emails::{
    EmailMessage, EmailProvider, local_test::LocalTestEmailProvider, ses::SesEmailProvider,
};

static PAST_DUE_TEMPLATE: Lazy<Handlebars<'static>> = Lazy::new(|| {
    let mut hbs = Handlebars::new();
    hbs.register_template_string("billing_past_due", include_str!("billing_past_due.hbs"))
        .expect("billing_past_due.hbs is valid");
    hbs
});

pub struct PastDueEmail<'a> {
    pub to_email: &'a str,
    pub org_name: &'a str,
    pub org_slug: &'a str,
    pub grace_ends_at: DateTime<Utc>,
    /// Public base URL of the deployment (`OXY_PUBLIC_URL` or the StripeConfig
    /// equivalent). Used to build the "Manage billing" link.
    pub public_url: &'a str,
}

pub async fn send_past_due_email(args: PastDueEmail<'_>) -> Result<(), OxyError> {
    let Some(config) = oxy::config::oxy::get_oxy_config()
        .ok()
        .and_then(|c| c.authentication)
        .and_then(|a| a.magic_link)
    else {
        tracing::warn!(
            "Past-due email not sent — magic-link email config missing. Org owner will not be notified about the start of the grace window."
        );
        return Ok(());
    };

    // Billing is a section of the settings dialog, opened by `?settings=`
    // (`useSettingsDeepLink`); there is no `/billing/plans` route.
    let billing_url = format!(
        "{}/{}?settings=organization.billing",
        args.public_url, args.org_slug
    );
    let grace_human = args
        .grace_ends_at
        .format("%B %-d, %Y at %H:%M UTC")
        .to_string();

    let subject = format!("Action needed: {} subscription is past due", args.org_name);
    let text_body = format!(
        "Your latest invoice for {org} couldn't be charged.\n\nYou have until {when} to update the payment method — after that, write access will be paused until the invoice is paid. Reads keep working in the meantime.\n\nUpdate payment method:\n{url}\n",
        org = args.org_name,
        when = grace_human,
        url = billing_url,
    );

    let message = EmailMessage {
        subject,
        html_body: build_html(&billing_url, args.org_name, &grace_human)?,
        text_body,
    };

    if std::env::var("MAGIC_LINK_LOCAL_TEST").is_ok() {
        LocalTestEmailProvider
            .send(&config.from_email, args.to_email, message)
            .await
    } else {
        SesEmailProvider::new(config.aws_region.as_deref())
            .await
            .send(&config.from_email, args.to_email, message)
            .await
    }
}

fn build_html(billing_url: &str, org_name: &str, grace_human: &str) -> Result<String, OxyError> {
    let data = serde_json::json!({
        "billing_url": billing_url,
        "org_name": org_name,
        "grace_ends_at_human": grace_human,
        "year": Utc::now().format("%Y").to_string(),
    });
    PAST_DUE_TEMPLATE
        .render("billing_past_due", &data)
        .map_err(|e| OxyError::RuntimeError(format!("Failed to render past-due template: {e}")))
}
