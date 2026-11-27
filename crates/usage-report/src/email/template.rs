//! The weekly custom-app usage report's mail body: what it looks like.
//!
//! Presentation only. What goes in the report — which apps, which sentences —
//! is decided in [`super`], which hands this module a [`ReportEmail`] already
//! narrowed to what its reader may see. How the mail leaves (SES, a browser
//! preview, or nothing) is `oxy_app_core::emails::usage_report`'s.

use chrono::Utc;
use handlebars::Handlebars;
use once_cell::sync::Lazy;
use oxy_shared::errors::OxyError;
use serde::Serialize;

use oxy_shared::email::EmailMessage;

static TEMPLATE: Lazy<Handlebars<'static>> = Lazy::new(|| {
    let mut hbs = Handlebars::new();
    hbs.register_template_string("usage_report", include_str!("usage_report.hbs"))
        .expect("usage_report.hbs is valid");
    hbs
});

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
#[path = "template_tests.rs"]
mod tests;
