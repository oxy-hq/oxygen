//! Which store an environment's events wait in, and which environment a
//! store vouches for (ruling R12): **the relay never trusts a row's
//! `environment` across stores.**
//!
//! - **Production's store** holds production's events only: a row there
//!   claiming any other environment is stored as production's with a relay
//!   error and never delivered.
//! - **A branch store** (the org's OLTP staging branch) holds the events of
//!   the non-production environments it hosts: staging, and each sandbox of
//!   the publishing app whose `ctx.oltp` runs in a schema of its own there
//!   (its planes' `oltp` is `schema`). A row claiming anything else —
//!   production, another app's sandbox, one on an OLTP branch of its own, a
//!   name that is no environment — is stored as staging's, the branch's own
//!   environment, with a relay error, and never delivered.
//! - **A sandbox's own branch** (its planes' `oltp` is `branch` or `empty`)
//!   holds that one sandbox's events: a row its app wrote claiming that
//!   sandbox. Anything else — staging, production, another sandbox, another
//!   app's row — is stored as that sandbox's with a relay error, and never
//!   delivered.
//!
//! Which store an environment's events wait in is `app_events::stores`'.
//!
//! A branch is a copy of production, so it starts with production's rows: one
//! still waiting when the branch was cut claims `production` and is refused
//! here, never delivered twice.

use std::collections::HashMap;

use entity::{app_environments, apps};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_custom_apps::planes::{OltpMode, planes_from_row};
use oxy_oltp::events::OutboxTarget;
use sea_orm::{DatabaseConnection, DbErr, EntityTrait};
use uuid::Uuid;

const PRODUCTION: &str = "production";
/// The environment a branch store's events are stored in when the row's own
/// claim is refused: the branch is staging's.
const BRANCH_HOME: &str = "staging";

/// The environment an event is stored in, and why the row's own claim was
/// refused, when it was.
pub(crate) type Vouched = (String, Option<String>);

/// What a store vouches for, asked once per `(app, claimed environment)` in a
/// pass.
pub(crate) struct Vouch<'a> {
    db: &'a DatabaseConnection,
    target: &'a OutboxTarget,
    seen: HashMap<(Uuid, String), Vouched>,
}

impl<'a> Vouch<'a> {
    pub(crate) fn new(db: &'a DatabaseConnection, target: &'a OutboxTarget) -> Self {
        Self {
            db,
            target,
            seen: HashMap::new(),
        }
    }

    /// The environment a row `app` wrote into this store, claiming `claimed`,
    /// is stored in.
    pub(crate) async fn environment(
        &mut self,
        app: &apps::Model,
        claimed: &str,
    ) -> Result<Vouched, DbErr> {
        match self.target {
            OutboxTarget::Production => return Ok(production(claimed)),
            OutboxTarget::EnvBranch {
                app_id,
                environment,
            } => {
                let home = (*app_id, environment.as_str());
                return Ok(own_branch(home, (app.id, &app.slug), claimed));
            }
            OutboxTarget::Branch(_) => {}
        }
        let key = (app.id, claimed.to_string());
        if let Some(vouched) = self.seen.get(&key) {
            return Ok(vouched.clone());
        }
        let hosted = hosted_here(self.db, app.id, claimed).await?;
        let vouched = on_branch(&app.slug, claimed, hosted);
        self.seen.insert(key, vouched.clone());
        Ok(vouched)
    }
}

/// Production's store: production, whatever the row claims.
pub(crate) fn production(claimed: &str) -> Vouched {
    let refused = (claimed != PRODUCTION).then(|| {
        format!(
            "published to production's outbox claiming environment `{claimed}`; production's \
             store holds production's events only"
        )
    });
    (PRODUCTION.to_string(), refused)
}

/// A sandbox's own branch, `home` (the app whose sandbox it is, and the
/// sandbox): the row's own claim when it is exactly that sandbox, written by
/// that app; the sandbox's otherwise, refused. The branch is a copy of
/// production at its cut (or production's structure), so a production row
/// waiting then is refused here, never delivered twice.
fn own_branch(home: (Uuid, &str), writer: (Uuid, &str), claimed: &str) -> Vouched {
    let ((owner, sandbox), (app_id, slug)) = (home, writer);
    if app_id == owner && claimed == sandbox {
        return (claimed.to_string(), None);
    }
    let refused = if app_id == owner {
        format!(
            "published to {slug}'s own OLTP branch for {sandbox} claiming environment \
             `{claimed}`; a sandbox's own branch holds that sandbox's events only"
        )
    } else {
        format!(
            "published by {slug} to another app's own OLTP branch for {sandbox}, claiming \
             environment `{claimed}`; a sandbox's own branch holds that sandbox's events only"
        )
    };
    (sandbox.to_string(), Some(refused))
}

/// The staging branch: the row's own claim when it is staging or, as
/// `hosted` says, a sandbox of the app `slug` on the branch; staging's
/// otherwise.
fn on_branch(slug: &str, claimed: &str, hosted: bool) -> Vouched {
    if claimed == BRANCH_HOME || hosted {
        return (claimed.to_string(), None);
    }
    let refused = format!(
        "published to the org's OLTP staging branch claiming environment `{claimed}`, which is \
         neither staging nor a sandbox of {slug} on that branch; a branch holds the events of the \
         environments it hosts only"
    );
    (BRANCH_HOME.to_string(), Some(refused))
}

/// Whether `claimed` is a sandbox of `app_id` whose OLTP lives on the org's
/// staging branch: it exists, and its planes keep `ctx.oltp` in a schema
/// there. A sandbox on an OLTP branch of its own, or whose planes no one can
/// read, is not.
async fn hosted_here(db: &DatabaseConnection, app_id: Uuid, claimed: &str) -> Result<bool, DbErr> {
    let Some(environment @ AppEnvironment::Dev { .. }) = AppEnvironment::parse(claimed) else {
        return Ok(false);
    };
    let Some(row) = app_environments::Entity::find_by_id((app_id, claimed.to_string()))
        .one(db)
        .await?
    else {
        return Ok(false);
    };
    Ok(matches!(
        planes_from_row(Some(&row), &environment),
        Ok(Some(planes)) if planes.oltp == OltpMode::Schema
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_vouches_for_production_only() {
        assert_eq!(production("production"), (PRODUCTION.to_string(), None));
        for claimed in ["staging", "dev-x", ""] {
            let (environment, error) = production(claimed);
            assert_eq!(environment, PRODUCTION, "the store decides");
            assert!(error.is_some(), "{claimed:?} is refused");
        }
    }

    /// R12 on a branch: staging and a sandbox hosted there stand; anything
    /// else is staging's, refused, naming the app.
    #[test]
    fn a_branch_vouches_for_staging_and_its_own_sandboxes() {
        assert_eq!(
            on_branch("locations", "staging", false),
            ("staging".into(), None)
        );
        assert_eq!(
            on_branch("locations", "dev-x", true),
            ("dev-x".into(), None)
        );
        for claimed in ["production", "dev-other", "nowhere", ""] {
            let (environment, error) = on_branch("locations", claimed, false);
            assert_eq!(environment, BRANCH_HOME, "{claimed:?}");
            let error = error.unwrap_or_else(|| panic!("{claimed:?} is refused"));
            assert!(error.contains(&format!("`{claimed}`")), "{error}");
            assert!(error.contains("locations"), "{error}");
        }
    }

    /// A sandbox's own branch holds that sandbox's events only: a row its
    /// app wrote claiming it stands; one claiming any other environment, or
    /// written by another app, is the sandbox's, refused.
    #[test]
    fn a_sandbox_branch_vouches_for_its_own_sandbox_only() {
        let owner = Uuid::from_u128(1);
        let home = (owner, "dev-x");
        assert_eq!(
            own_branch(home, (owner, "locations"), "dev-x"),
            ("dev-x".into(), None)
        );
        for claimed in ["staging", "production", "dev-y", ""] {
            let (environment, error) = own_branch(home, (owner, "locations"), claimed);
            assert_eq!(environment, "dev-x", "{claimed:?}");
            let error = error.unwrap_or_else(|| panic!("{claimed:?} is refused"));
            assert!(error.contains(&format!("`{claimed}`")), "{error}");
        }
        let other = (Uuid::from_u128(2), "billing");
        let (environment, error) = own_branch(home, other, "dev-x");
        assert_eq!(environment, "dev-x");
        let error = error.expect("another app's row is refused");
        assert!(error.contains("billing"), "{error}");
    }
}
