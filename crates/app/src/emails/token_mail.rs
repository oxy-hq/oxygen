//! How API-token mail leaves (API-tokens design §5, §8 Phase 5): the grant an
//! org revoked, the expiry notice, the leak revocation.
//!
//! Same sender as the other transactional mails — the magic-link SES identity
//! — through one function, [`deliver`]:
//!
//! - **Preview** when `MAGIC_LINK_LOCAL_TEST` or `OXY_APP_EMAIL_LOCAL_TEST` is
//!   set: the rendered mail is written to a temp file and opened, never sent,
//!   and recorded in this process's [`outbox`] — the sink the tests read.
//! - Otherwise **SES**, from the magic-link `from_email`.
//! - With no magic-link config, a logged no-op: what the mail reports has
//!   already happened and is audited either way.
//!
//! Every token mail names the token by its name and non-secret prefix. Never
//! the token.

use std::sync::{LazyLock, Mutex};

use handlebars::Handlebars;
use oxy_shared::errors::OxyError;
use serde_json::json;

use crate::emails::{
    EmailMessage, EmailProvider, local_test::LocalTestEmailProvider, ses::SesEmailProvider,
};

/// One previewed mail, as the outbox keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Previewed {
    pub to: String,
    pub subject: String,
    pub text_body: String,
}

/// The most previews the outbox keeps; older ones fall off.
const OUTBOX_MAX: usize = 200;

static OUTBOX: LazyLock<Mutex<Vec<Previewed>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// The token mails this process previewed, oldest first. Empty unless a
/// preview variable is set: production sends, and keeps nothing.
pub fn outbox() -> Vec<Previewed> {
    OUTBOX.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

fn record(to: &str, message: &EmailMessage) {
    let mut outbox = OUTBOX.lock().unwrap_or_else(|p| p.into_inner());
    if outbox.len() >= OUTBOX_MAX {
        outbox.remove(0);
    }
    outbox.push(Previewed {
        to: to.to_string(),
        subject: message.subject.clone(),
        text_body: message.text_body.clone(),
    });
}

/// Whether mail is previewed locally rather than sent.
pub fn previews_locally() -> bool {
    ["MAGIC_LINK_LOCAL_TEST", "OXY_APP_EMAIL_LOCAL_TEST"]
        .iter()
        .any(|name| std::env::var(name).is_ok())
}

/// Send one token mail, or preview it. See the module docs.
pub async fn deliver(to: &str, message: EmailMessage) -> Result<(), OxyError> {
    let config = oxy::config::oxy::get_oxy_config()
        .ok()
        .and_then(|c| c.authentication)
        .and_then(|a| a.magic_link);
    if previews_locally() {
        record(to, &message);
        let from = config.map(|c| c.from_email).unwrap_or_default();
        return LocalTestEmailProvider.send(&from, to, message).await;
    }
    let Some(config) = config else {
        tracing::warn!(
            subject = %message.subject,
            "token mail not sent — magic-link email config missing"
        );
        return Ok(());
    };
    SesEmailProvider::new(config.aws_region.as_deref())
        .await
        .send(&config.from_email, to, message)
        .await
}

/// Who a token mail is for: a person's own credential (a personal token, or a
/// legacy API key — which links to its own page, [`legacy_keys_url`]), or an
/// org's service-account token, whose mail goes to the org's admins and owners.
#[derive(Clone, Copy, Debug)]
pub enum Audience<'a> {
    Owner,
    OrgAdmins { org_name: &'a str, account: &'a str },
}

impl Audience<'_> {
    /// Where the reader acts on the token: their own token list, or the org's
    /// API access settings. `None` when the deployment's URL is unknown
    /// (`OXY_API_URL` unset); the mail then names the place in words.
    pub fn settings_url(&self) -> Option<String> {
        let base = oxy_app_core::custom_apps_host_dispatch::admin_base_url()?;
        Some(format!("{base}/?settings={}", self.settings_key()))
    }

    /// The `?settings=` value of the page that lists the token.
    pub fn settings_key(&self) -> &'static str {
        match self {
            Self::Owner => "account.tokens",
            Self::OrgAdmins { .. } => "organization.api_access",
        }
    }

    /// The place, in words, for a mail with no link.
    pub fn settings_label(&self) -> &'static str {
        match self {
            Self::Owner => "Account → Personal access tokens",
            Self::OrgAdmins { .. } => "Organization → API access",
        }
    }
}

/// The `?settings=` value of the page that lists a person's legacy API keys.
/// A legacy key is not a token: it is extended and revoked in its own section.
pub const LEGACY_KEYS_SETTINGS_KEY: &str = "workspace.legacy_api_keys";

/// That page, in words, for a mail with no link.
pub const LEGACY_KEYS_SETTINGS_LABEL: &str = "Workspace → Legacy API keys";

/// Where a legacy API key's owner acts on it. `None` when the deployment's URL
/// is unknown (`OXY_API_URL` unset).
pub fn legacy_keys_url() -> Option<String> {
    let base = oxy_app_core::custom_apps_host_dispatch::admin_base_url()?;
    Some(format!("{base}/?settings={LEGACY_KEYS_SETTINGS_KEY}"))
}

const TEMPLATE: &str = r#"<!DOCTYPE html>
<html lang="en">
<body style="margin:0;padding:32px 16px;background-color:#f4f4f5;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,Helvetica,Arial,sans-serif;color:#18181b;">
  <div style="max-width:520px;margin:0 auto;background-color:#ffffff;border:1px solid #e4e4e7;border-radius:16px;padding:36px 40px;">
    <h1 style="margin:0 0 16px;font-size:22px;line-height:1.3;">{{title}}</h1>
    {{#each paragraphs}}<p style="margin:0 0 12px;font-size:15px;line-height:1.6;">{{this}}</p>
    {{/each}}{{#if link}}<p style="margin:20px 0 0;"><a href="{{link}}" style="display:inline-block;padding:10px 18px;background-color:#18181b;color:#ffffff;border-radius:8px;text-decoration:none;font-size:14px;">{{link_label}}</a></p>{{/if}}
  </div>
</body>
</html>"#;

static RENDERER: LazyLock<Handlebars<'static>> = LazyLock::new(|| {
    let mut hbs = Handlebars::new();
    hbs.register_template_string("token_mail", TEMPLATE)
        .expect("the token_mail template is valid");
    hbs
});

/// A plain token mail: a title, paragraphs, and an optional button. Every
/// value is HTML-escaped — names are user-controlled text.
pub struct Plain<'a> {
    pub subject: String,
    pub title: String,
    pub paragraphs: Vec<String>,
    pub link: Option<(&'a str, String)>,
}

impl Plain<'_> {
    pub fn message(self) -> Result<EmailMessage, OxyError> {
        let (link_label, link) = self.link.as_ref().map_or((None, None), |(label, url)| {
            (Some(*label), Some(url.clone()))
        });
        let data = json!({
            "title": self.title,
            "paragraphs": self.paragraphs,
            "link": link,
            "link_label": link_label,
        });
        let html_body = RENDERER
            .render("token_mail", &data)
            .map_err(|e| OxyError::RuntimeError(format!("Failed to render token mail: {e}")))?;
        let mut text_body = self.paragraphs.join("\n\n");
        if let Some((label, url)) = &self.link {
            text_body.push_str(&format!("\n\n{label}: {url}"));
        }
        text_body.push('\n');
        Ok(EmailMessage {
            subject: self.subject,
            html_body,
            text_body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_mail_escapes_and_carries_its_link_in_both_bodies() {
        let mail = Plain {
            subject: "s".into(),
            title: "Acme <Corp>".into(),
            paragraphs: vec!["one <b>".into(), "two".into()],
            link: Some((
                "Extend it",
                "https://oxy.example/?settings=account.tokens".into(),
            )),
        }
        .message()
        .unwrap();
        assert!(mail.html_body.contains("Acme &lt;Corp&gt;"));
        assert!(!mail.html_body.contains("one <b>"));
        assert!(
            mail.html_body
                .contains("https://oxy.example/?settings&#x3D;account.tokens")
                || mail
                    .html_body
                    .contains("https://oxy.example/?settings=account.tokens")
        );
        assert_eq!(
            mail.text_body,
            "one <b>\n\ntwo\n\nExtend it: https://oxy.example/?settings=account.tokens\n"
        );
    }

    #[test]
    fn the_outbox_keeps_the_newest_previews() {
        for i in 0..OUTBOX_MAX + 3 {
            record(
                "a@b.c",
                &EmailMessage {
                    subject: format!("{i}"),
                    html_body: String::new(),
                    text_body: String::new(),
                },
            );
        }
        let kept = outbox();
        assert_eq!(kept.len(), OUTBOX_MAX);
        assert_eq!(kept.last().unwrap().subject, format!("{}", OUTBOX_MAX + 2));
    }
}
