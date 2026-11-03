//! Environment pointers: `app_environments` plus its `app_environment_events`
//! history (`internal-docs/2026-09-10-custom-app-environments-design.md` §3).
//!
//! **`app_environments.build_id` is the only store of which build an
//! environment serves.** [`record_move`] is the only writer: every move of
//! production's, staging's or a sandbox's pointer goes through it, in the
//! transaction that holds the app's row (`lock_app`), and appends the event.
//!
//! **Sandboxes** (`dev-<handle>`, `internal-docs/custom-app-sandboxes.md`): their
//! row *is* the pointer and is created by the sandbox API. [`record_move`] moves
//! it and appends the same event, but never creates the row.

use std::collections::HashMap;

use chrono::{DateTime, FixedOffset, Utc};
use entity::{app_environment_events, app_environments};
use oxy_app_core::custom_app_environment::{AppEnvironment, AppEnvironmentKind};
use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter,
    QuerySelect,
};
use uuid::Uuid;

/// Why a pointer moved. Mirrors the `app_environment_events.action` CHECK
/// (`backfill` is written only by the migration).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnvAction {
    Publish,
    Promote,
    Rollback,
    Unpublish,
    Reset,
}

impl EnvAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Publish => "publish",
            Self::Promote => "promote",
            Self::Rollback => "rollback",
            Self::Unpublish => "unpublish",
            Self::Reset => "reset",
        }
    }
}

/// Point `environment` of `app_id` at `build_id` (`None` means it serves nothing)
/// and append the event. Run it inside a transaction that holds the app's row
/// (`custom_apps_promote::production::lock_app`), so two moves of one app never
/// interleave.
///
/// **A fixed environment's row is upserted; a sandbox's is only ever moved.** A
/// `dev-<handle>` row needs an owner and is created by the sandbox API
/// (`custom_apps_sandboxes::ops::create`), never as a side effect of a pointer
/// move. So for a sandbox this updates the existing row that is not being torn
/// down, and answers [`DbErr::RecordNotFound`] — writing nothing, no event
/// either — when there is none: the sandbox was never created, or a delete got
/// there first.
pub async fn record_move<C: ConnectionTrait>(
    conn: &C,
    app_id: Uuid,
    environment: &AppEnvironment,
    build_id: Option<Uuid>,
    action: EnvAction,
    actor: Option<Uuid>,
) -> Result<(), DbErr> {
    let now = Utc::now().fixed_offset();
    match environment {
        AppEnvironment::Dev { .. } => {
            move_sandbox(conn, app_id, environment, build_id, actor, now).await?
        }
        _ => upsert_fixed(conn, app_id, environment, build_id, actor, now).await?,
    }

    app_environment_events::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        app_id: ActiveValue::Set(app_id),
        environment: ActiveValue::Set(environment.name()),
        build_id: ActiveValue::Set(build_id),
        action: ActiveValue::Set(action.as_str().to_string()),
        actor: ActiveValue::Set(actor),
        at: ActiveValue::Set(now),
    }
    .insert(conn)
    .await?;

    Ok(())
}

/// `production` or `staging`: insert the row, or move the one that exists.
async fn upsert_fixed<C: ConnectionTrait>(
    conn: &C,
    app_id: Uuid,
    environment: &AppEnvironment,
    build_id: Option<Uuid>,
    actor: Option<Uuid>,
    now: DateTime<FixedOffset>,
) -> Result<(), DbErr> {
    app_environments::Entity::insert(app_environments::ActiveModel {
        app_id: ActiveValue::Set(app_id),
        name: ActiveValue::Set(environment.name()),
        kind: ActiveValue::Set(environment.kind().as_str().to_string()),
        owner_user_id: ActiveValue::Set(None),
        build_id: ActiveValue::Set(build_id),
        updated_by: ActiveValue::Set(actor),
        updated_at: ActiveValue::Set(now),
        created_at: ActiveValue::Set(now),
        deleting_at: ActiveValue::Set(None),
        oltp_schema: ActiveValue::NotSet,
        created_by_token_id: ActiveValue::NotSet,
        // Never moved by a pointer write: a fixed environment's first row
        // takes the default (off), and a later move leaves the switch alone.
        schedules_enabled: ActiveValue::NotSet,
        // Branch facts: nothing writes them yet, so the row takes the defaults.
        parent_name: ActiveValue::NotSet,
        fork_at: ActiveValue::NotSet,
        planes: ActiveValue::NotSet,
        semantic_revision_id: ActiveValue::NotSet,
        semantic_git_sha: ActiveValue::NotSet,
    })
    .on_conflict(
        OnConflict::columns([
            app_environments::Column::AppId,
            app_environments::Column::Name,
        ])
        .update_columns([
            app_environments::Column::BuildId,
            app_environments::Column::UpdatedBy,
            app_environments::Column::UpdatedAt,
        ])
        .to_owned(),
    )
    .exec_without_returning(conn)
    .await?;
    Ok(())
}

/// A sandbox: move the pointer of the row that exists and is not being torn
/// down. No such row is [`DbErr::RecordNotFound`].
async fn move_sandbox<C: ConnectionTrait>(
    conn: &C,
    app_id: Uuid,
    environment: &AppEnvironment,
    build_id: Option<Uuid>,
    actor: Option<Uuid>,
    now: DateTime<FixedOffset>,
) -> Result<(), DbErr> {
    let moved = app_environments::Entity::update_many()
        .col_expr(app_environments::Column::BuildId, Expr::value(build_id))
        .col_expr(app_environments::Column::UpdatedBy, Expr::value(actor))
        .col_expr(app_environments::Column::UpdatedAt, Expr::value(now))
        .filter(app_environments::Column::AppId.eq(app_id))
        .filter(app_environments::Column::Name.eq(environment.name()))
        .filter(app_environments::Column::Kind.eq(AppEnvironmentKind::Dev.as_str()))
        .filter(app_environments::Column::DeletingAt.is_null())
        .exec(conn)
        .await?;
    if moved.rows_affected == 0 {
        return Err(DbErr::RecordNotFound(format!(
            "app {app_id} has no sandbox {environment} (it was never created, or it is being \
             deleted)"
        )));
    }
    Ok(())
}

/// Every build some environment of `app_id` currently serves. `gc_builds` must
/// never reap one of these.
pub async fn protected_build_ids<C: ConnectionTrait>(
    conn: &C,
    app_id: Uuid,
) -> Result<Vec<Uuid>, DbErr> {
    let builds: Vec<Option<Uuid>> = app_environments::Entity::find()
        .select_only()
        .column(app_environments::Column::BuildId)
        .filter(app_environments::Column::AppId.eq(app_id))
        .into_tuple()
        .all(conn)
        .await?;
    Ok(builds.into_iter().flatten().collect())
}

/// The builds an app's fixed environments serve. `None`: the environment serves
/// nothing (no row, or a row with no build).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FixedPointers {
    pub production: Option<Uuid>,
    pub staging: Option<Uuid>,
}

impl FixedPointers {
    fn set(&mut self, name: &str, build: Option<Uuid>) {
        match name {
            "production" => self.production = build,
            "staging" => self.staging = build,
            _ => {}
        }
    }
}

/// The builds `production` and `staging` of `app_id` serve, from their rows.
pub async fn fixed_pointers<C: ConnectionTrait>(
    conn: &C,
    app_id: Uuid,
) -> Result<FixedPointers, DbErr> {
    let rows = app_environments::Entity::find()
        .filter(app_environments::Column::AppId.eq(app_id))
        .filter(app_environments::Column::Name.is_in(["production", "staging"]))
        .all(conn)
        .await?;
    let mut pointers = FixedPointers::default();
    for row in rows {
        pointers.set(&row.name, row.build_id);
    }
    Ok(pointers)
}

/// [`fixed_pointers`] for many apps in **one** query — for list endpoints,
/// which must not pay a query per row. Every id asked for is in the map; an
/// app with no rows maps to [`FixedPointers::default`].
pub async fn fixed_pointers_batch<C: ConnectionTrait>(
    conn: &C,
    app_ids: &[Uuid],
) -> Result<HashMap<Uuid, FixedPointers>, DbErr> {
    let mut pointers: HashMap<Uuid, FixedPointers> = app_ids
        .iter()
        .map(|id| (*id, FixedPointers::default()))
        .collect();
    if app_ids.is_empty() {
        return Ok(pointers);
    }
    let rows = app_environments::Entity::find()
        .filter(app_environments::Column::AppId.is_in(app_ids.iter().copied()))
        .filter(app_environments::Column::Name.is_in(["production", "staging"]))
        .all(conn)
        .await?;
    for row in rows {
        if let Some(entry) = pointers.get_mut(&row.app_id) {
            entry.set(&row.name, row.build_id);
        }
    }
    Ok(pointers)
}

/// The build one environment of `app_id` serves right now: its row's
/// `build_id`, or `None` when there is no row or it points nowhere. A sandbox
/// being torn down (`deleting_at` set) serves nothing.
pub async fn pointer<C: ConnectionTrait>(
    conn: &C,
    app_id: Uuid,
    environment: &AppEnvironment,
) -> Result<Option<Uuid>, DbErr> {
    let row = app_environments::Entity::find_by_id((app_id, environment.name()))
        .one(conn)
        .await?;
    Ok(row
        .filter(|row| row.deleting_at.is_none())
        .and_then(|row| row.build_id))
}
