//! What the Apps tab reads, for one app or every app of an org, in the same
//! fixed number of queries: production pointers, live builds, approvals,
//! accounts, live rows and people's names, each read once for all the apps.

use std::collections::HashMap;

use entity::prelude::{AppBuilds, Users};
use entity::{api_tokens, app_builds, app_platform_grants, apps, service_accounts, users};
use oxy_auth::token::app_access as store;
use oxy_custom_apps::env_resolve::{load_environment_builds, load_environment_builds_batch};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::dto::{AppAccessDto, LiveBuildDto, actions_of, rfc3339};
use crate::user_tokens::error::TokenError;

/// Every row the page needs for a set of apps, keyed for assembly.
pub(super) struct Loaded {
    /// App → the build production runs.
    builds: HashMap<Uuid, app_builds::Model>,
    /// App → its live approvals, oldest first.
    grants: HashMap<Uuid, Vec<app_platform_grants::Model>>,
    /// App → the service account it acts as.
    accounts: HashMap<Uuid, service_accounts::Model>,
    /// Account → its live row of kind `app`.
    tokens: HashMap<Uuid, api_tokens::Model>,
    people: HashMap<Uuid, String>,
}

impl Loaded {
    /// One app's DTO, from the rows read for it.
    pub(super) fn dto(&self, app: &apps::Model) -> AppAccessDto {
        let build = self.builds.get(&app.id);
        let account = self.accounts.get(&app.id);
        let token = account.and_then(|a| self.tokens.get(&a.user_id));
        let grants = self.grants.get(&app.id).map(Vec::as_slice).unwrap_or(&[]);
        let person = |id: Option<Uuid>| id.and_then(|id| self.people.get(&id).cloned());
        AppAccessDto {
            app_id: app.id,
            app_slug: app.slug.clone(),
            app_name: app.name.clone(),
            live_build: build.map(|b| LiveBuildDto {
                build_id: b.build_id.clone(),
                published_at: rfc3339(b.created_at),
                published_by: person(b.published_by),
                published_via: b.published_via.clone(),
            }),
            actions: actions_of(
                build.and_then(|b| b.manifest_json.as_ref()),
                grants,
                token.map(|t| t.id),
                &self.people,
            ),
            token_id: token.map(|t| t.id),
            service_account: account.map(|a| a.name.clone()),
            last_used_at: token.and_then(|t| t.last_used_at).map(rfc3339),
        }
    }
}

/// Read everything [`Loaded::dto`] needs for `apps`.
pub(super) async fn load(
    db: &DatabaseConnection,
    apps: &[apps::Model],
) -> Result<Loaded, TokenError> {
    let ids: Vec<Uuid> = apps.iter().map(|a| a.id).collect();
    let pointers = load_environment_builds_batch(db, apps).await?;
    // Build → app. `production_runs` reads only a cached mark, and only for an
    // app whose production has no build of its own.
    let mut app_of: HashMap<Uuid, Uuid> = HashMap::new();
    for app in apps {
        if let Some(pointers) = pointers.get(&app.id)
            && let Some(build) = pointers.production_runs(db).await?
        {
            app_of.insert(build, app.id);
        }
    }
    let builds: HashMap<Uuid, app_builds::Model> = if app_of.is_empty() {
        HashMap::new()
    } else {
        AppBuilds::find()
            .filter(app_builds::Column::Id.is_in(app_of.keys().copied()))
            .all(db)
            .await?
            .into_iter()
            .filter_map(|b| app_of.get(&b.id).map(|app| (*app, b)))
            .collect()
    };
    let grants = store::approvals_of(db, &ids).await?;
    let accounts = store::accounts_of(db, &ids).await?;
    let account_ids: Vec<Uuid> = accounts.values().map(|a| a.user_id).collect();
    let tokens = store::live_tokens(db, &account_ids).await?;
    let people = names(
        db,
        grants
            .values()
            .flatten()
            .filter_map(|g| g.created_by)
            .chain(builds.values().filter_map(|b| b.published_by)),
    )
    .await?;
    Ok(Loaded {
        builds,
        grants,
        accounts,
        tokens,
        people,
    })
}

/// The live build of `app` and its manifest, if it has one: what an approval
/// is checked against.
pub(super) async fn live_build(
    db: &DatabaseConnection,
    app: &apps::Model,
) -> Result<Option<app_builds::Model>, TokenError> {
    let Some(build_id) = load_environment_builds(db, app)
        .await?
        .production_runs(db)
        .await?
    else {
        return Ok(None);
    };
    Ok(AppBuilds::find_by_id(build_id).one(db).await?)
}

/// A person's display name: their name, else their address.
async fn names(
    db: &DatabaseConnection,
    ids: impl IntoIterator<Item = Uuid>,
) -> Result<HashMap<Uuid, String>, TokenError> {
    let mut ids: Vec<Uuid> = ids.into_iter().collect();
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    Ok(Users::find()
        .filter(users::Column::Id.is_in(ids))
        .all(db)
        .await?
        .into_iter()
        .map(|u| {
            let label = if u.name.trim().is_empty() {
                u.email.clone().unwrap_or_default()
            } else {
                u.name.clone()
            };
            (u.id, label)
        })
        .collect())
}
