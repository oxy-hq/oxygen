//! "An organization ended your API token's access to it." Sent to a personal
//! token's owner when an org admin revokes the org's grant on it from the org
//! token inventory (API-tokens design §5).
//!
//! Sent through [`token_mail::deliver`]: SES from the magic-link identity, a
//! local preview under `MAGIC_LINK_LOCAL_TEST` / `OXY_APP_EMAIL_LOCAL_TEST`,
//! and a logged no-op with no magic-link config — the grant is revoked and
//! audited either way, and the owner still sees it in their token list.
//!
//! The mail names the token by its name and non-secret prefix. Never the
//! token.

use handlebars::Handlebars;
use once_cell::sync::Lazy;
use oxy_shared::errors::OxyError;

use crate::emails::{EmailMessage, token_mail};

const TEMPLATE: &str = r#"<!DOCTYPE html>
<html lang="en">
<body style="margin:0;padding:32px 16px;background-color:#f4f4f5;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,Helvetica,Arial,sans-serif;color:#18181b;">
  <div style="max-width:520px;margin:0 auto;background-color:#ffffff;border:1px solid #e4e4e7;border-radius:16px;padding:36px 40px;">
    <h1 style="margin:0 0 16px;font-size:22px;line-height:1.3;">{{org_name}} ended an API token's access</h1>
    <p style="margin:0 0 12px;font-size:15px;line-height:1.6;">
      An administrator of <strong>{{org_name}}</strong> ({{revoked_by}}) ended the access your token
      <strong>{{token_name}}</strong> (<code>{{display_prefix}}…</code>) had to that organization.
    </p>
    <p style="margin:0 0 12px;font-size:15px;line-height:1.6;">
      The token itself still works everywhere else it could reach. Anything using it against
      {{org_name}} will now be refused.
    </p>
    <p style="margin:0;font-size:13px;line-height:1.6;color:#71717a;">
      You can see this under Account → Personal access tokens. To reach {{org_name}} again, ask one of
      its administrators, then create a new token.
    </p>
  </div>
</body>
</html>"#;

static RENDERER: Lazy<Handlebars<'static>> = Lazy::new(|| {
    let mut hbs = Handlebars::new();
    hbs.register_template_string("token_grant_revoked", TEMPLATE)
        .expect("the token_grant_revoked template is valid");
    hbs
});

pub struct GrantRevokedEmail<'a> {
    pub to_email: &'a str,
    pub org_name: &'a str,
    pub token_name: &'a str,
    /// The token's non-secret leading fragment, e.g. `oxy_pat_Ab3x`.
    pub display_prefix: &'a str,
    /// Who revoked it, as a label.
    pub revoked_by: &'a str,
}

fn message(args: &GrantRevokedEmail<'_>) -> Result<EmailMessage, OxyError> {
    let data = serde_json::json!({
        "org_name": args.org_name,
        "token_name": args.token_name,
        "display_prefix": args.display_prefix,
        "revoked_by": args.revoked_by,
    });
    let html_body = RENDERER
        .render("token_grant_revoked", &data)
        .map_err(|e| OxyError::RuntimeError(format!("Failed to render grant-revoked mail: {e}")))?;
    let text_body = format!(
        "An administrator of {org} ({by}) ended the access your API token \"{token}\" ({prefix}…) \
         had to that organization.\n\nThe token itself still works everywhere else it could reach. \
         Anything using it against {org} will now be refused.\n\nYou can see this under Account → \
         Personal access tokens. To reach {org} again, ask one of its administrators, then create \
         a new token.\n",
        org = args.org_name,
        by = args.revoked_by,
        token = args.token_name,
        prefix = args.display_prefix,
    );
    Ok(EmailMessage {
        subject: format!("{} ended an API token's access", args.org_name),
        html_body,
        text_body,
    })
}

pub async fn send_grant_revoked_email(args: GrantRevokedEmail<'_>) -> Result<(), OxyError> {
    let message = message(&args)?;
    token_mail::deliver(args.to_email, message).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mail_names_the_token_and_the_org_and_escapes_them() {
        let mail = message(&GrantRevokedEmail {
            to_email: "ada@acme.com",
            org_name: "Acme <Corp>",
            token_name: "laptop",
            display_prefix: "oxy_pat_Ab3x",
            revoked_by: "grace@acme.com",
        })
        .unwrap();
        assert_eq!(mail.subject, "Acme <Corp> ended an API token's access");
        assert!(mail.text_body.contains("\"laptop\" (oxy_pat_Ab3x…)"));
        assert!(mail.text_body.contains("grace@acme.com"));
        // HTML-escaped: an org name is user-controlled text.
        assert!(mail.html_body.contains("Acme &lt;Corp&gt;"));
        assert!(!mail.html_body.contains("Acme <Corp>"));
        assert!(mail.html_body.contains("oxy_pat_Ab3x"));
    }
}
