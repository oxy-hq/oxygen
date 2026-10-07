//! The weekly custom-app usage report, as mail: what it looks like, and how it
//! leaves.
//!
//! Presentation only. What goes in the report — which apps, which sentences —
//! is decided in `server::api::admin::usage_report`, which hands this module a
//! [`ReportEmail`] already narrowed to what its reader may see.
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

use chrono::Utc;
use handlebars::Handlebars;
use once_cell::sync::Lazy;
use oxy_shared::errors::OxyError;
use serde::Serialize;

use crate::emails::{
    EmailMessage, EmailProvider, local_test::LocalTestEmailProvider, ses::SesEmailProvider,
    token_mail,
};

static TEMPLATE: Lazy<Handlebars<'static>> = Lazy::new(|| {
    let mut hbs = Handlebars::new();
    hbs.register_template_string("usage_report", include_str!("usage_report.hbs"))
        .expect("usage_report.hbs is valid");
    hbs
});

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

/// One number at the top of the mail: `People 35 −1`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EmailStat {
    pub label: String,
    pub value: String,
    /// A change or a qualifier shown small beside the value (`+6`, `31
    /// failed`); empty for none.
    pub note: String,
}

/// One app worth a line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EmailHighlight {
    pub app: String,
    pub org: String,
    /// What happened, in two or three words: `Went quiet`.
    pub label: String,
    /// The numbers behind it: `5 people → nobody`.
    pub detail: String,
}

/// One row of the table. The three optional columns arrive as text, already
/// formatted; a column the mail does not show is left empty.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EmailOrg {
    pub name: String,
    pub people: u64,
    /// People against the week before: `+3`, `−2`; empty for no change.
    pub change: String,
    pub views: String,
    pub calls: String,
    pub releases: String,
    pub storage: String,
}

/// The report as its reader sees it. Lists are already cut to what a mail
/// should carry; each `*_more` says how many were left for the full report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReportEmail {
    pub subject: String,
    pub period: String,
    pub headline: String,
    /// The numbers under the headline, three to a row.
    pub stat_rows: Vec<Vec<EmailStat>>,
    pub attention: Vec<EmailHighlight>,
    pub attention_more: usize,
    pub good: Vec<EmailHighlight>,
    pub good_more: usize,
    pub orgs: Vec<EmailOrg>,
    pub orgs_more: usize,
    /// Which optional columns the table has: one nothing can fill is left out.
    pub show_calls: bool,
    pub show_releases: bool,
    pub show_storage: bool,
    /// Apps that are live and unopened, as one sentence fragment; empty for none.
    pub idle: String,
    /// The deployment's host, so prod's mail and staging's are told apart.
    pub host: Option<String>,
    pub report_url: Option<String>,
    pub settings_url: Option<String>,
}

pub fn render(mail: &ReportEmail) -> Result<EmailMessage, OxyError> {
    let mut data = serde_json::to_value(mail)
        .map_err(|e| OxyError::RuntimeError(format!("Failed to build usage report mail: {e}")))?;
    data["year"] = Utc::now().format("%Y").to_string().into();
    let html_body = TEMPLATE.render("usage_report", &data).map_err(|e| {
        OxyError::RuntimeError(format!("Failed to render usage report template: {e}"))
    })?;
    Ok(EmailMessage {
        subject: mail.subject.clone(),
        html_body,
        text_body: text_body(mail),
    })
}

fn text_body(mail: &ReportEmail) -> String {
    let mut out = format!("Custom app usage, {}\n{}\n", mail.period, mail.headline);
    let stats: Vec<String> = mail.stat_rows.iter().flatten().map(text_stat).collect();
    if !stats.is_empty() {
        out.push_str(&format!("{}\n", stats.join(" | ")));
    }
    if mail.attention.is_empty() {
        out.push_str("\nNothing needs a look this week.\n");
    }
    text_highlights(
        &mut out,
        "Needs a look",
        &mail.attention,
        mail.attention_more,
    );
    text_highlights(&mut out, "Going well", &mail.good, mail.good_more);
    if !mail.orgs.is_empty() {
        out.push_str("\nBy organization\n");
        for org in &mail.orgs {
            out.push_str(&format!("- {}\n", text_org(mail, org)));
        }
        push_more(&mut out, mail.orgs_more);
    }
    if !mail.idle.is_empty() {
        out.push_str(&format!("\nNot opened in two weeks: {}\n", mail.idle));
    }
    match &mail.report_url {
        Some(url) => out.push_str(&format!("\nFull report: {url}\n")),
        None => out.push_str("\nFull report: Admin, then Usage report.\n"),
    }
    match &mail.settings_url {
        Some(url) => out.push_str(&format!("Stop these emails: {url}\n")),
        None => out.push_str("Stop these emails: Admin, then Settings under your name.\n"),
    }
    out
}

/// `People 35 (−1)`.
fn text_stat(stat: &EmailStat) -> String {
    if stat.note.is_empty() {
        format!("{} {}", stat.label, stat.value)
    } else {
        format!("{} {} ({})", stat.label, stat.value, stat.note)
    }
}

/// `Northwind: 19 people (+6), 156 opens, 240 calls, 2 releases, 2.1 GB`.
fn text_org(mail: &ReportEmail, org: &EmailOrg) -> String {
    let mut line = format!("{}: {} people", org.name, org.people);
    if !org.change.is_empty() {
        line.push_str(&format!(" ({})", org.change));
    }
    line.push_str(&format!(", {} opens", org.views));
    let counted = |count: &str, one: &str, many: &str| {
        format!(", {count} {}", if count == "1" { one } else { many })
    };
    if mail.show_calls {
        line.push_str(&counted(&org.calls, "call", "calls"));
    }
    if mail.show_releases {
        line.push_str(&counted(&org.releases, "release", "releases"));
    }
    if mail.show_storage {
        line.push_str(&format!(", {}", org.storage));
    }
    line
}

fn text_highlights(out: &mut String, title: &str, items: &[EmailHighlight], more: usize) {
    if items.is_empty() {
        return;
    }
    out.push_str(&format!("\n{title}\n"));
    for item in items {
        out.push_str(&format!(
            "- {} ({}): {}, {}\n",
            item.app, item.org, item.label, item.detail
        ));
    }
    push_more(out, more);
}

fn push_more(out: &mut String, more: usize) {
    if more > 0 {
        out.push_str(&format!("- And {more} more in the full report.\n"));
    }
}

#[cfg(test)]
#[path = "usage_report_tests.rs"]
mod tests;
