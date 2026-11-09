//! "Your API token expires in a week." Sent once per token per expiry, 7 days
//! ahead, by the token sweeper (API-tokens design §3.6, §8 Phase 5) — for a
//! personal token or legacy key to its owner, for a service-account token to
//! its org's admins and owners. Legacy keys included: the mail only informs.
//!
//! Plain on purpose: what expires, when, what stops working, and the link to
//! Extend — which keeps the secret, so nothing needs redeploying. Sent through
//! [`token_mail::deliver`].

use chrono::{DateTime, Utc};
use oxy_shared::errors::OxyError;

use crate::emails::EmailMessage;
use crate::emails::token_mail::{self, Audience, Plain};

pub struct ExpiringEmail<'a> {
    pub token_name: &'a str,
    /// The token's non-secret leading fragment, e.g. `oxy_pat_Ab3x`.
    pub display_prefix: &'a str,
    /// A legacy `oxy_<hex>` key: called a legacy API key, as its page calls it,
    /// and sent to that page — never to the token list.
    pub legacy: bool,
    pub expires_at: DateTime<Utc>,
    pub audience: Audience<'a>,
    /// The Extend page; `None` names it in words instead.
    pub extend_url: Option<String>,
}

fn when(at: DateTime<Utc>) -> String {
    at.format("%B %-d, %Y at %H:%M UTC").to_string()
}

fn day(at: DateTime<Utc>) -> String {
    at.format("%B %-d, %Y").to_string()
}

pub(crate) fn message(args: &ExpiringEmail<'_>) -> Result<EmailMessage, OxyError> {
    let (article, noun) = if args.legacy {
        ("A", "legacy API key")
    } else {
        ("An", "API token")
    };
    let (subject, opening, who_extends) = match args.audience {
        Audience::Owner => (
            format!(
                "Your {noun} \"{}\" expires on {}",
                args.token_name,
                day(args.expires_at)
            ),
            format!(
                "Your {noun} \"{}\" ({}…) expires on {}.",
                args.token_name,
                args.display_prefix,
                when(args.expires_at)
            ),
            "You can extend it".to_string(),
        ),
        Audience::OrgAdmins { org_name, account } => (
            format!(
                "A service-account token in {org_name} expires on {}",
                day(args.expires_at)
            ),
            format!(
                "The token \"{}\" ({}…) of the service account {account} in {org_name} expires on {}.",
                args.token_name,
                args.display_prefix,
                when(args.expires_at)
            ),
            format!("An admin of {org_name} can extend it"),
        ),
    };
    let place = match &args.extend_url {
        Some(_) => String::new(),
        None if args.legacy => format!(" under {}", token_mail::LEGACY_KEYS_SETTINGS_LABEL),
        None => format!(" under {}", args.audience.settings_label()),
    };
    Plain {
        subject,
        title: format!("{article} {noun} expires in a week"),
        paragraphs: vec![
            opening,
            "After that, anything still using it — a script, a CI job, an integration — will be \
             refused."
                .to_string(),
            format!(
                "{who_extends}{place}. Extending keeps the same secret, so nothing needs \
                 redeploying. If it is no longer needed, you can let it expire."
            ),
        ],
        link: args.extend_url.clone().map(|url| {
            let label = if args.legacy {
                "Extend the key"
            } else {
                "Extend the token"
            };
            (label, url)
        }),
    }
    .message()
}

pub async fn send_expiring_email(to_email: &str, args: &ExpiringEmail<'_>) -> Result<(), OxyError> {
    token_mail::deliver(to_email, message(args)?).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-10T14:30:00Z")
            .unwrap()
            .into()
    }

    #[test]
    fn the_owner_is_told_what_expires_when_and_where_to_extend() {
        let mail = message(&ExpiringEmail {
            token_name: "laptop",
            display_prefix: "oxy_pat_Ab3x",
            legacy: false,
            expires_at: at(),
            audience: Audience::Owner,
            extend_url: Some("https://oxy.example/?settings=account.tokens".into()),
        })
        .unwrap();
        assert_eq!(
            mail.subject,
            "Your API token \"laptop\" expires on October 10, 2026"
        );
        assert!(
            mail.text_body
                .contains("(oxy_pat_Ab3x…) expires on October 10, 2026 at 14:30 UTC")
        );
        assert!(mail.text_body.contains("will be refused"));
        assert!(
            mail.text_body
                .contains("https://oxy.example/?settings=account.tokens")
        );
    }

    #[test]
    fn a_legacy_key_is_called_a_legacy_api_key_and_an_accounts_token_goes_to_its_org() {
        let legacy = message(&ExpiringEmail {
            token_name: "deploy",
            display_prefix: "oxy_1234",
            legacy: true,
            expires_at: at(),
            audience: Audience::Owner,
            extend_url: None,
        })
        .unwrap();
        assert!(legacy.subject.starts_with("Your legacy API key \"deploy\""));
        assert!(
            legacy
                .text_body
                .contains("under Workspace → Legacy API keys")
        );
        assert!(!legacy.text_body.contains("Personal access tokens"));

        let account = message(&ExpiringEmail {
            token_name: "release",
            display_prefix: "oxy_sat_Zz9q",
            legacy: false,
            expires_at: at(),
            audience: Audience::OrgAdmins {
                org_name: "Acme",
                account: "deploy-bot",
            },
            extend_url: Some("https://oxy.example/?settings=organization.api_access".into()),
        })
        .unwrap();
        assert_eq!(
            account.subject,
            "A service-account token in Acme expires on October 10, 2026"
        );
        assert!(
            account
                .text_body
                .contains("service account deploy-bot in Acme")
        );
        assert!(account.text_body.contains("An admin of Acme can extend it"));
        assert!(
            account
                .text_body
                .contains("settings=organization.api_access")
        );
    }
}
