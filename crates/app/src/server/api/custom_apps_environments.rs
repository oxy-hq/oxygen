//! Environment pointer writes: `app_environments` plus its `app_environment_events`
//! history (`internal-docs/2026-09-10-custom-app-environments-design.md` §3).
//!
//! **Phase 1a contract.** Every write to `apps.draft_build_id` (staging) or
//! `apps.published_build_id` (production) also calls [`record_move`], on the same
//! connection or transaction, so the rows can never disagree with the columns
//! readers still use. `tests/custom_apps/app_environment_pointer_writes.rs`
//! enforces this mechanically. Call it by its qualified path,
//! `custom_apps_environments::record_move(`, because that is the spelling the test
//! looks for.

use chrono::Utc;
use entity::{app_environment_events, app_environments};
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::sea_query::OnConflict;
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
/// and append the event. Run it inside the transaction that moves the `apps`
/// pointer it mirrors.
///
/// Refuses dev slots: their rows need an owner and are created by the dev-slot API
/// (spec §6), never as a side effect of a pointer move.
pub async fn record_move<C: ConnectionTrait>(
    conn: &C,
    app_id: Uuid,
    environment: &AppEnvironment,
    build_id: Option<Uuid>,
    action: EnvAction,
    actor: Option<Uuid>,
) -> Result<(), DbErr> {
    if matches!(environment, AppEnvironment::Dev { .. }) {
        return Err(DbErr::Custom(format!(
            "record_move does not create dev slots ({environment}); use the dev-slot API"
        )));
    }
    let now = Utc::now().fixed_offset();

    app_environments::Entity::insert(app_environments::ActiveModel {
        app_id: ActiveValue::Set(app_id),
        name: ActiveValue::Set(environment.name()),
        kind: ActiveValue::Set(environment.kind().as_str().to_string()),
        owner_user_id: ActiveValue::Set(None),
        build_id: ActiveValue::Set(build_id),
        updated_by: ActiveValue::Set(actor),
        updated_at: ActiveValue::Set(now),
        created_at: ActiveValue::Set(now),
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
