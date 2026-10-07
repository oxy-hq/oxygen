//! The **staging option** of a sandbox agent token (`oxy_sbx_`): who may mint
//! with it, and how a client learns this server takes it (sandbox agent
//! credential design, "Staging option").
//!
//! A mint may say `"staging": true`. The token is then also granted the
//! staging environment of **every** app it names — a draft publish, and the
//! loop on staging — stored as one `app_staging` grant beside each app's
//! `app_sandbox` one (`oxy_auth::token::sandbox`). It never reaches
//! production, and never promotes.
//!
//! ## Who may mint with it
//!
//! Whoever may mint the token at all (`sandbox::check`), **and** may open the
//! staging of each app it names: `Action::AppNonProduction` on that app's
//! staging, decided by oxy-authz with the caller's standing read past the
//! grant cache as the `existing_allow`. Asked wherever the mint is checked —
//! `POST /api/user/tokens`, the CLI's `authorize` under the session, and
//! again at its `exchange`.
//!
//! An app whose staging the caller may not open answers what an app they may
//! not mint for answers: 404 `app_not_found`, naming it. The mint cannot be
//! used to learn which apps have a staging the caller cannot see.

use entity::apps;
use oxy_app_core::audit::RequestActor;
use oxy_authz::{Action, Cap, EnvFacet, Resource};
use sea_orm::DatabaseConnection;
use serde::Serialize;

use super::error::TokenError;
use super::sandbox::{SandboxAgentLimits, standing_of};
use crate::server::authz;

/// The label of this decision in the authz trace.
const LABEL: &str = "sandbox_token_mint_staging";

/// Hold a mint that asks for staging to apps whose staging `actor` may open
/// right now. Call only for such a mint: one without `staging` asks nothing
/// here, and is checked exactly as it was.
pub(super) async fn check(
    db: &DatabaseConnection,
    actor: &RequestActor,
    apps: &[apps::Model],
) -> Result<(), TokenError> {
    let caller = authz::caller_of(actor);
    let standing = standing_of(db, actor).await?;
    for app in apps {
        // The shipped rule that opens staging, read uncached: `develop_apps`
        // over the app's org. The model can only subtract from it.
        let existing_allow = standing.reaches(Cap::DevelopApps, app.org_id);
        let staging = Resource::app(app.id, app.org_id).in_environment(EnvFacet::Staging);
        let action = Action::AppNonProduction;
        if !authz::enforce_for(db, &caller, LABEL, action, staging, existing_allow).await {
            return Err(TokenError::AppNotFound(app.id.to_string()));
        }
    }
    Ok(())
}

/// What `GET /api/user/token-options` says of a sandbox agent mint: its
/// limits, and that this server takes `staging`. The limits are flattened, so
/// the object a client has always read gains one key.
#[derive(Debug, PartialEq, Serialize)]
pub struct SandboxAgentOptions {
    #[serde(flatten)]
    pub limits: SandboxAgentLimits,
    /// Always `true` here. A server one release back sends no such key, and a
    /// client reads its absence as "do not offer staging".
    pub staging: bool,
}

impl SandboxAgentOptions {
    pub(super) fn current() -> Self {
        Self {
            limits: SandboxAgentLimits::current(),
            staging: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// The dialog's object keeps its three limits and gains `staging`.
    #[test]
    fn the_options_are_the_limits_and_that_staging_is_taken() {
        let options = serde_json::to_value(SandboxAgentOptions::current()).unwrap();
        assert_eq!(
            options,
            json!({ "default_hours": 8, "max_hours": 168, "max_apps": 5, "staging": true })
        );
    }
}
