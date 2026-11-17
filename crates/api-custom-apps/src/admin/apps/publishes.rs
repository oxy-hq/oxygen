//! `GET /api/customer-apps/{id}/publishes?limit=` — the app's publish
//! attempts, newest first: the newest `limit` that failed beside the newest
//! `limit` that landed (each outcome is kept, and read, in a window of its
//! own), each with how long its stages took and, for a failure, the stage and
//! the error (`custom_apps_publish_attempts`).
//!
//! Beside `GET /{id}/builds` and under the same guard stack: the build ledger
//! answers what landed, this what was tried. Every environment's attempts, as
//! the ledger lists every environment's builds; each names its own.

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use entity::app_publish_attempts;
use entity::prelude::{Apps, Users};
use oxy::database::client::establish_connection;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::custom_apps_publish_attempts::{self as attempts, StageTiming, timings_from_json};

/// `?limit=` on the listing: how many attempts of each outcome, newest first.
/// Defaults to [`attempts::DEFAULT_LIST_LIMIT`], capped at
/// [`attempts::MAX_LIST_LIMIT`] — all that retention keeps.
#[derive(Debug, Default, Deserialize)]
pub struct PublishesQuery {
    pub limit: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct PublishAttemptList {
    pub attempts: Vec<PublishAttemptSummary>,
}

/// One attempt, as the console reads it.
#[derive(Debug, Serialize)]
pub struct PublishAttemptSummary {
    pub id: Uuid,
    /// The build row a successful attempt recorded (`app_builds.id`, what the
    /// build ledger lists); null for a failure, or once retention deleted it.
    pub app_build_id: Option<Uuid>,
    pub build_id: String,
    pub environment: String,
    /// `success` or `failed`.
    pub outcome: String,
    /// The stage a failed attempt failed at.
    pub failed_stage: Option<String>,
    /// The refusal a failed attempt answered, in full as stored.
    pub error: Option<String>,
    pub started_at: String,
    pub finished_at: String,
    /// Wall time from the first stage's start to the record.
    pub duration_ms: i64,
    /// Every stage that ran, in pipeline order.
    pub stages: Vec<StageTiming>,
    /// Who published, as the build ledger says it.
    pub published_by_email: Option<String>,
    pub published_via: Option<String>,
    pub published_token_id: Option<Uuid>,
}

impl PublishAttemptSummary {
    /// `row`, with its publisher's email when there is one to show.
    pub fn of(row: app_publish_attempts::Model, published_by_email: Option<String>) -> Self {
        let duration_ms = (row.finished_at - row.started_at).num_milliseconds().max(0);
        Self {
            id: row.id,
            app_build_id: row.app_build_id,
            build_id: row.build_id,
            environment: row.environment,
            outcome: row.outcome,
            failed_stage: row.failed_stage,
            error: row.error,
            started_at: row.started_at.to_rfc3339(),
            finished_at: row.finished_at.to_rfc3339(),
            duration_ms,
            stages: timings_from_json(&row.stage_ms),
            published_by_email,
            published_via: row.published_via,
            published_token_id: row.published_token_id,
        }
    }
}

/// See the module docs. 404 for an app that does not exist.
pub async fn list_publishes(
    Path(id): Path<Uuid>,
    Query(query): Query<PublishesQuery>,
) -> Result<Json<PublishAttemptList>, StatusCode> {
    let db = establish_connection().await.map_err(|e| {
        tracing::error!("list_publishes DB connect failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let failed = |e: sea_orm::DbErr| {
        tracing::error!("list_publishes query failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    };
    Apps::find_by_id(id)
        .one(&db)
        .await
        .map_err(failed)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let rows = attempts::recent(&db, id, attempts::clamp_limit(query.limit))
        .await
        .map_err(failed)?;
    let emails = publisher_emails(&db, &rows).await.map_err(failed)?;
    let attempts = rows
        .into_iter()
        .map(|row| {
            let email = row.published_by.and_then(|uid| emails.get(&uid).cloned());
            PublishAttemptSummary::of(row, email)
        })
        .collect();
    Ok(Json(PublishAttemptList { attempts }))
}

/// The label of every user who published one of `rows`, in one query.
async fn publisher_emails(
    db: &DatabaseConnection,
    rows: &[app_publish_attempts::Model],
) -> Result<HashMap<Uuid, String>, sea_orm::DbErr> {
    let mut ids: Vec<Uuid> = rows.iter().filter_map(|r| r.published_by).collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    Ok(Users::find()
        .filter(entity::users::Column::Id.is_in(ids))
        .all(db)
        .await?
        .into_iter()
        .map(|u| (u.id, u.label().to_string()))
        .collect())
}

#[cfg(test)]
#[path = "publishes_tests.rs"]
mod tests;
