//! CRUD operations for the analytics run extension table.

use sea_orm::sea_query::OnConflict;
use sea_orm::{ActiveValue::*, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use serde_json::Value;

use super::entity;

/// Insert an analytics extension row for a run.
pub async fn insert_extension(
    db: &DatabaseConnection,
    run_id: &str,
    agent_id: &str,
    thinking_mode: Option<String>,
) -> Result<(), DbErr> {
    let model = entity::ActiveModel {
        run_id: Set(run_id.to_string()),
        agent_id: Set(agent_id.to_string()),
        spec_hint: Set(None),
        thinking_mode: Set(thinking_mode),
        // Left out of the insert: an attempt stamps them when it begins
        // executing (`super::execution::begin_execution`), never the seed.
        execution_started_at: NotSet,
        execution_heartbeat_at: NotSet,
    };
    match entity::Entity::insert(model)
        .on_conflict(
            OnConflict::column(entity::Column::RunId)
                .do_nothing()
                .to_owned(),
        )
        .exec(db)
        .await
    {
        Ok(_) | Err(DbErr::RecordNotInserted) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Update the spec_hint on the extension row after pipeline completion.
pub async fn update_spec_hint(
    db: &DatabaseConnection,
    run_id: &str,
    spec_hint: Value,
) -> Result<(), DbErr> {
    let model = entity::ActiveModel {
        run_id: Set(run_id.to_string()),
        spec_hint: Set(Some(spec_hint)),
        ..Default::default()
    };
    entity::Entity::update(model).exec(db).await?;
    Ok(())
}

pub async fn update_thinking_mode(
    db: &DatabaseConnection,
    run_id: &str,
    thinking_mode: Option<String>,
) -> Result<(), DbErr> {
    let model = entity::ActiveModel {
        run_id: Set(run_id.to_string()),
        thinking_mode: Set(thinking_mode),
        ..Default::default()
    };
    entity::Entity::update(model).exec(db).await?;
    Ok(())
}

/// Load the extension row for a single run.
pub async fn get_extension(
    db: &DatabaseConnection,
    run_id: &str,
) -> Result<Option<entity::Model>, DbErr> {
    entity::Entity::find_by_id(run_id.to_string()).one(db).await
}

/// Load extension rows for multiple run IDs (bulk fetch for thread queries).
pub async fn get_extensions_by_run_ids(
    db: &DatabaseConnection,
    run_ids: &[String],
) -> Result<Vec<entity::Model>, DbErr> {
    if run_ids.is_empty() {
        return Ok(vec![]);
    }
    entity::Entity::find()
        .filter(entity::Column::RunId.is_in(run_ids.iter().map(|s| s.as_str())))
        .all(db)
        .await
}
