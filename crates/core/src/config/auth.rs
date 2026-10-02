use garde::Validate;
use serde::{Deserialize, Serialize};
use std::env;

use schemars::JsonSchema;

#[derive(Serialize, Deserialize, Validate, Debug, Clone, JsonSchema)]
pub struct Authentication {
    #[garde(dive)]
    pub google: Option<GoogleAuth>,
    #[garde(dive)]
    pub okta: Option<OktaAuth>,
    #[garde(dive)]
    pub magic_link: Option<MagicLinkAuth>,
}

#[derive(Serialize, Deserialize, Validate, Debug, Clone, JsonSchema)]
pub struct MagicLinkAuth {
    /// Verified SES sender email address
    #[garde(length(min = 1))]
    pub from_email: String,
    /// AWS region for SES (defaults to AWS_REGION env var)
    #[garde(skip)]
    pub aws_region: Option<String>,
    /// Allow specific individual emails (for closed beta)
    #[garde(skip)]
    #[serde(default)]
    pub allowed_emails: Vec<String>,
}

#[derive(Serialize, Deserialize, Validate, Debug, Clone, JsonSchema)]
pub struct GoogleAuth {
    #[garde(length(min = 1))]
    pub client_id: String,
    #[garde(length(min = 1))]
    pub client_secret: String,
}

#[derive(Serialize, Deserialize, Validate, Debug, Clone, JsonSchema)]
pub struct OktaAuth {
    #[garde(length(min = 1))]
    pub client_id: String,
    #[garde(length(min = 1))]
    pub client_secret: String,
    #[garde(length(min = 1))]
    pub domain: String,
}

/// `MAGIC_LINK_BLOCKED_DOMAINS` used to refuse whole mailbox providers
/// (gmail.com and the like). Sign-in no longer filters by domain, so a
/// deployment still carrying the variable is told it does nothing rather than
/// left believing the block is in force.
fn warn_if_domain_block_list_is_set() {
    if env::var("MAGIC_LINK_BLOCKED_DOMAINS").is_ok_and(|v| !v.trim().is_empty()) {
        tracing::warn!(
            "MAGIC_LINK_BLOCKED_DOMAINS is set but no longer honored — magic-link sign-in \
             does not filter by email domain. Remove it from this deployment's environment."
        );
    }
}

impl Authentication {
    pub fn from_env() -> Result<Self, oxy_shared::errors::OxyError> {
        let client_id = env::var("GOOGLE_CLIENT_ID").ok();
        let client_secret = env::var("GOOGLE_CLIENT_SECRET").ok();
        let google = match (client_id, client_secret) {
            (Some(id), Some(secret)) => Some(GoogleAuth {
                client_id: id,
                client_secret: secret,
            }),
            _ => None,
        };

        let okta_client_id = env::var("OKTA_CLIENT_ID").ok();
        let okta_client_secret = env::var("OKTA_CLIENT_SECRET").ok();
        let okta_domain = env::var("OKTA_DOMAIN").ok();
        let okta = match (okta_client_id, okta_client_secret, okta_domain) {
            (Some(id), Some(secret), Some(domain)) => Some(OktaAuth {
                client_id: id,
                client_secret: secret,
                domain,
            }),
            _ => None,
        };

        let magic_link_local_test = env::var("MAGIC_LINK_LOCAL_TEST").is_ok();
        let magic_link_from_email = env::var("MAGIC_LINK_FROM_EMAIL").ok();
        let magic_link = if magic_link_local_test || magic_link_from_email.is_some() {
            warn_if_domain_block_list_is_set();
            Some(MagicLinkAuth {
                from_email: magic_link_from_email
                    .unwrap_or_else(|| "noreply@localhost".to_string()),
                aws_region: env::var("MAGIC_LINK_AWS_REGION").ok(),
                allowed_emails: env::var("MAGIC_LINK_ALLOWED_EMAILS")
                    .unwrap_or_default()
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect(),
            })
        } else {
            None
        };

        let auth = Authentication {
            google,
            okta,
            magic_link,
        };

        Ok(auth)
    }
}
