//! "An API token was revoked because it was exposed." Sent when a leak report
//! (`POST /api/auth/tokens/revoke-leaked`) revokes a new-format token — to
//! the owner of a personal token, to the org's admins and owners for a
//! service-account token (API-tokens design §8 Phase 5). A legacy key is never
//! revoked this way, so never mailed by it.
//!
//! Sent through [`token_mail::deliver`].

use oxy_shared::errors::OxyError;

use crate::emails::EmailMessage;
use crate::emails::token_mail::{self, Audience, Plain};

pub struct LeakedEmail<'a> {
    pub token_name: &'a str,
    pub display_prefix: &'a str,
    pub audience: Audience<'a>,
    /// Who reported it (`github`, say), as the report said.
    pub source: Option<&'a str>,
    /// Where it was found, as the report said.
    pub url: Option<&'a str>,
    /// The token list; `None` names it in words instead.
    pub settings_url: Option<String>,
}

/// The report's own words, bounded: it is unauthenticated text.
fn bounded(s: &str) -> String {
    s.chars().take(300).collect()
}

pub(crate) fn message(args: &LeakedEmail<'_>) -> Result<EmailMessage, OxyError> {
    let whose = match args.audience {
        Audience::Owner => format!(
            "Your API token \"{}\" ({}…)",
            args.token_name, args.display_prefix
        ),
        Audience::OrgAdmins { org_name, account } => format!(
            "The token \"{}\" ({}…) of the service account {account} in {org_name}",
            args.token_name, args.display_prefix
        ),
    };
    let found_at = args
        .url
        .map(|u| format!(" at {}", bounded(u)))
        .unwrap_or_default();
    let reported_by = args
        .source
        .map(|s| format!(" by {}", bounded(s)))
        .unwrap_or_default();
    let place = match &args.settings_url {
        Some(_) => String::new(),
        None => format!(" under {}", args.audience.settings_label()),
    };
    Plain {
        subject: "An API token was revoked because it was exposed".into(),
        title: "An exposed API token was revoked".into(),
        paragraphs: vec![
            format!(
                "{whose} was reported as publicly exposed{found_at}{reported_by}, so it has been \
                 revoked."
            ),
            "Anything still using it is now refused. Create a new token to replace it, and remove \
             the exposed copy from wherever it was found."
                .to_string(),
            format!("You can see the revoked token{place}."),
        ],
        link: args
            .settings_url
            .clone()
            .map(|url| ("Open your tokens", url)),
    }
    .message()
}

pub async fn send_leaked_email(to_email: &str, args: &LeakedEmail<'_>) -> Result<(), OxyError> {
    token_mail::deliver(to_email, message(args)?).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mail_says_what_was_revoked_where_it_was_found_and_what_to_do() {
        let mail = message(&LeakedEmail {
            token_name: "laptop",
            display_prefix: "oxy_pat_Ab3x",
            audience: Audience::Owner,
            source: Some("github"),
            url: Some("https://github.com/acme/app/blob/main/.env"),
            settings_url: None,
        })
        .unwrap();
        assert!(mail.text_body.contains(
            "Your API token \"laptop\" (oxy_pat_Ab3x…) was reported as publicly exposed at \
             https://github.com/acme/app/blob/main/.env by github, so it has been revoked."
        ));
        assert!(mail.text_body.contains("Create a new token"));
        assert!(
            mail.text_body
                .contains("under Account → Personal access tokens")
        );
    }

    #[test]
    fn an_accounts_token_names_the_account_and_bounds_the_report() {
        let long = "x".repeat(1000);
        let mail = message(&LeakedEmail {
            token_name: "release",
            display_prefix: "oxy_sat_Zz9q",
            audience: Audience::OrgAdmins {
                org_name: "Acme",
                account: "deploy-bot",
            },
            source: None,
            url: Some(&long),
            settings_url: Some("https://oxy.example/?settings=organization.api_access".into()),
        })
        .unwrap();
        assert!(
            mail.text_body
                .contains("service account deploy-bot in Acme")
        );
        assert!(!mail.text_body.contains(&long));
        assert!(mail.text_body.contains("settings=organization.api_access"));
    }
}
