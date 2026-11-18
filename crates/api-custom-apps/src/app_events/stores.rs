//! Which store an environment's events wait in (app events design §6.2,
//! §6.7): production's tenant database for production; the org's OLTP staging
//! branch for staging and for each sandbox whose `ctx.oltp` runs in a schema
//! there; and a sandbox on an OLTP branch of its own (its planes' `oltp` is
//! `branch` or `empty`) that branch, [`OutboxTarget::EnvBranch`].
//!
//! A sandbox's store is its app's choice (its planes), so a `dev-*` name
//! alone no longer says which store: [`store_of`] reads the publishing app's
//! environment row, and [`relayed_stores`] every app's of that name in the
//! org. The relay, a delivery's body read, retention, purge, replay and the
//! single-event route all ask here.

use std::collections::HashMap;

use entity::{app_environments, apps};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_custom_apps::planes::{OltpMode, planes_from_row};
use oxy_oltp::events::OutboxTarget;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QuerySelect,
};
use uuid::Uuid;

/// Whether `environment` names one whose events are relayed: production,
/// staging or a sandbox. A name that is no environment has no store.
pub(crate) fn is_relayed(environment: &str) -> bool {
    AppEnvironment::parse(environment).is_some()
}

/// The store the events `app_id` publishes in `environment` wait in, and so
/// where their bodies are kept; for an event the platform publishes itself
/// (no app: a table change), the store every app shares there
/// ([`shared_store_of`]). `None` for a name that is no environment.
pub(crate) async fn store_of(
    db: &impl ConnectionTrait,
    app_id: Option<Uuid>,
    environment: &str,
) -> Result<Option<OutboxTarget>, DbErr> {
    let Some(parsed) = AppEnvironment::parse(environment) else {
        return Ok(None);
    };
    let Some(app_id) = app_id.filter(|_| matches!(parsed, AppEnvironment::Dev { .. })) else {
        return Ok(Some(shared_store(&parsed, environment)));
    };
    let row = app_environments::Entity::find_by_id((app_id, environment.to_string()))
        .one(db)
        .await?;
    Ok(Some(sandbox_store(
        app_id,
        row.as_ref(),
        &parsed,
        environment,
    )))
}

/// Every store `org_id`'s `environment` outbox may hold rows in: production's
/// for production, the staging branch for staging, and for a sandbox the
/// staging branch (its schema sandboxes') and the own branch of every app
/// whose sandbox of that name is on one. Empty for a name that is no
/// environment.
pub(crate) async fn relayed_stores(
    db: &DatabaseConnection,
    org_id: Uuid,
    environment: &str,
) -> Result<Vec<OutboxTarget>, DbErr> {
    let Some(parsed) = AppEnvironment::parse(environment) else {
        return Ok(Vec::new());
    };
    let shared = shared_store(&parsed, environment);
    if !matches!(parsed, AppEnvironment::Dev { .. }) {
        return Ok(vec![shared]);
    }
    let apps: Vec<Uuid> = apps::Entity::find()
        .select_only()
        .column(apps::Column::Id)
        .filter(apps::Column::OrgId.eq(org_id))
        .into_tuple()
        .all(db)
        .await?;
    let mut rows = app_environments::Entity::find()
        .filter(app_environments::Column::Name.eq(environment))
        .filter(app_environments::Column::AppId.is_in(apps))
        .all(db)
        .await?;
    rows.sort_by_key(|row| row.app_id);
    let mut stores = vec![shared];
    for row in &rows {
        let own = sandbox_store(row.app_id, Some(row), &parsed, environment);
        if matches!(own, OutboxTarget::EnvBranch { .. }) {
            stores.push(own);
        }
    }
    Ok(stores)
}

/// The store every app shares in `environment`: production's, or the
/// staging branch hosting staging and the schema sandboxes, where the
/// platform's own events (a table change) keep their bodies. `None` for a
/// name that is no environment.
pub(crate) fn shared_store_of(environment: &str) -> Option<OutboxTarget> {
    AppEnvironment::parse(environment).map(|parsed| shared_store(&parsed, environment))
}

/// The store every app shares for `parsed`: production's, or the staging
/// branch hosting staging and the schema sandboxes.
fn shared_store(parsed: &AppEnvironment, environment: &str) -> OutboxTarget {
    match parsed {
        AppEnvironment::Production => OutboxTarget::Production,
        AppEnvironment::Staging | AppEnvironment::Dev { .. } => {
            OutboxTarget::Branch(environment.to_string())
        }
    }
}

/// A sandbox's store: its own branch when its planes put `ctx.oltp` on one,
/// the staging branch otherwise (a schema there, the default, or planes no
/// one can read).
fn sandbox_store(
    app_id: Uuid,
    row: Option<&app_environments::Model>,
    parsed: &AppEnvironment,
    environment: &str,
) -> OutboxTarget {
    let own = matches!(
        planes_from_row(row, parsed),
        Ok(Some(planes)) if planes.oltp != OltpMode::Schema
    );
    if own {
        OutboxTarget::EnvBranch {
            app_id,
            environment: environment.to_string(),
        }
    } else {
        OutboxTarget::Branch(environment.to_string())
    }
}

/// [`store_of`], read once per `(app, environment)`: for a caller walking a
/// batch of stored events (retention, purge, replay, the event log).
pub(crate) struct Stores<'a, C: ConnectionTrait> {
    db: &'a C,
    seen: HashMap<(Option<Uuid>, String), Option<OutboxTarget>>,
}

impl<'a, C: ConnectionTrait> Stores<'a, C> {
    pub(crate) fn new(db: &'a C) -> Self {
        Self {
            db,
            seen: HashMap::new(),
        }
    }

    /// Where the body of an event `app_id` published in `environment` is
    /// (`None`: the platform published it).
    pub(crate) async fn of(
        &mut self,
        app_id: Option<Uuid>,
        environment: &str,
    ) -> Result<Option<OutboxTarget>, DbErr> {
        let key = (app_id, environment.to_string());
        if let Some(found) = self.seen.get(&key) {
            return Ok(found.clone());
        }
        let found = store_of(self.db, app_id, environment).await?;
        self.seen.insert(key, found.clone());
        Ok(found)
    }
}

#[cfg(test)]
mod tests;
