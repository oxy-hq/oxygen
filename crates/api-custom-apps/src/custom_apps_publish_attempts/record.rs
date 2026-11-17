//! Writing one publish attempt, and keeping an app's attempts bounded.
//!
//! The pipeline opens an [`Attempt`] before its first stage, clocks every
//! stage on it, tells it the app and the build as it learns them, and
//! [`Attempt::finish`]es it with the outcome. The write is best effort, as
//! every row written after the action it records: an attempt that cannot be
//! recorded is logged, and the publish it describes stands — or fails — as it
//! did.
//!
//! Retention follows the build rows'. `gc_builds` runs after each publish and
//! keeps the newest builds per window; [`prune`] runs after each attempt and
//! keeps the newest [`KEEP_ATTEMPTS`] per outcome, so a CI job failing in a
//! loop cannot push the timings of the builds that did land out of the
//! console, nor a long run of good publishes the last failures.

use chrono::{DateTime, Utc};
use entity::app_publish_attempts::{self, OUTCOME_FAILED, OUTCOME_SUCCESS};
use entity::apps;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder, QuerySelect,
};
use uuid::Uuid;

use super::stages::{Stage, StageTimer, StageTiming};

/// How many attempts an app keeps of each outcome.
pub const KEEP_ATTEMPTS: u64 = 50;

/// The longest refusal an attempt stores, in characters. A refusal is a
/// sentence or a validation report; anything longer is cut, and says so.
pub const MAX_ERROR_CHARS: usize = 4_000;
/// The longest build id an attempt stores: a valid one is at most 200
/// characters (`custom_apps_build_store::is_valid_build_id`), and a refusal
/// for an invalid one is recorded with it cut.
pub const MAX_BUILD_ID_CHARS: usize = 200;
/// The longest environment name or publisher label an attempt stores.
pub const MAX_LABEL_CHARS: usize = 512;

/// What a publish says of itself before it runs.
#[derive(Debug, Clone)]
pub struct AttemptFacts {
    /// The app the publish names: with the org it was authorized in, how a
    /// publish refused before its app row was read finds it.
    pub app_slug: String,
    pub build_id: String,
    /// The environment the publish moves.
    pub environment: String,
    /// The publisher, as `app_builds` records it.
    pub published_by: Option<Uuid>,
    pub published_via: Option<String>,
    pub published_token_id: Option<Uuid>,
}

/// One publish as it runs: what it said of itself, its clock, and what it
/// learned on the way.
#[derive(Debug)]
pub struct Attempt {
    facts: AttemptFacts,
    started_at: DateTime<Utc>,
    pub clock: StageTimer,
    /// The app the publish works on, once it is known **and** will outlive a
    /// failure: an app this publish created is undone with it, so it is set
    /// only once that publish stands.
    pub app_id: Option<Uuid>,
    /// The build row the publish recorded, once the publish stands.
    pub app_build_id: Option<Uuid>,
    /// The org the caller was authorized to publish this app into. `None`
    /// until `authorize_publish` let it through, and an attempt with no
    /// authorized org is never written: a caller refused authorization has
    /// no standing in any app's history.
    pub authorized_org: Option<Uuid>,
}

impl Attempt {
    pub fn begin(facts: AttemptFacts) -> Self {
        Self {
            facts,
            started_at: Utc::now(),
            clock: StageTimer::new(),
            app_id: None,
            app_build_id: None,
            authorized_org: None,
        }
    }

    /// The stage the publish failed at, if it failed in one.
    pub fn failed_stage(&self) -> Option<Stage> {
        self.clock.failed()
    }

    /// The stages that ran, in pipeline order.
    pub fn timings(&self) -> Vec<StageTiming> {
        self.clock.timings()
    }

    /// Record the attempt. `refused` is the refusal a failed publish answers,
    /// `None` for one that landed. Never fails: the publish already has its
    /// outcome, and an attempt that cannot be written is logged.
    pub async fn finish(&self, db: &DatabaseConnection, refused: Option<String>) {
        let Some(app_id) = self.app(db).await else {
            tracing::debug!(
                build_id = %self.facts.build_id,
                "publish attempt not recorded: no app to record it against"
            );
            return;
        };
        let row = self.row(app_id, refused.as_deref(), Utc::now());
        let outcome = row.outcome.clone();
        if let Err(e) = row.insert(db).await {
            tracing::warn!(
                app_id = %app_id,
                build_id = %self.facts.build_id,
                error = %e,
                "publish attempt not recorded"
            );
            return;
        }
        if let ActiveValue::Set(outcome) = outcome {
            prune(db, app_id, &outcome).await;
        }
    }

    /// The app to record against: none until the caller was authorized;
    /// then the one the publish resolved, else — refused between authorization
    /// and reading the app row — the app the authorization was for, the slug
    /// in the authorized org (`authorize_publish` decides on that same app).
    /// `None` when there is none to see it.
    async fn app(&self, db: &DatabaseConnection) -> Option<Uuid> {
        let org_id = self.authorized_org?;
        if let Some(app_id) = self.app_id {
            return Some(app_id);
        }
        let found = apps::Entity::find()
            .filter(apps::Column::OrgId.eq(org_id))
            .filter(apps::Column::Slug.eq(&self.facts.app_slug))
            .one(db)
            .await;
        match found {
            Ok(app) => app.map(|app| app.id),
            Err(e) => {
                tracing::warn!(error = %e, "publish attempt: could not look its app up");
                None
            }
        }
    }

    /// The row this attempt writes. A failed attempt names no build row: if
    /// the publish got as far as recording one, its undo deleted it.
    pub(super) fn row(
        &self,
        app_id: Uuid,
        refused: Option<&str>,
        finished_at: DateTime<Utc>,
    ) -> app_publish_attempts::ActiveModel {
        let (outcome, app_build_id, failed_stage) = match refused {
            None => (OUTCOME_SUCCESS, self.app_build_id, None),
            Some(_) => (
                OUTCOME_FAILED,
                None,
                self.failed_stage().map(|s| s.name().to_string()),
            ),
        };
        let facts = &self.facts;
        app_publish_attempts::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            app_id: ActiveValue::Set(app_id),
            app_build_id: ActiveValue::Set(app_build_id),
            build_id: ActiveValue::Set(capped(&facts.build_id, MAX_BUILD_ID_CHARS)),
            environment: ActiveValue::Set(capped(&facts.environment, MAX_LABEL_CHARS)),
            published_by: ActiveValue::Set(facts.published_by),
            published_via: ActiveValue::Set(
                facts
                    .published_via
                    .as_deref()
                    .map(|via| capped(via, MAX_LABEL_CHARS)),
            ),
            published_token_id: ActiveValue::Set(facts.published_token_id),
            started_at: ActiveValue::Set(self.started_at.fixed_offset()),
            finished_at: ActiveValue::Set(finished_at.fixed_offset()),
            outcome: ActiveValue::Set(outcome.to_string()),
            failed_stage: ActiveValue::Set(failed_stage),
            error: ActiveValue::Set(refused.map(|e| capped(e, MAX_ERROR_CHARS))),
            stage_ms: ActiveValue::Set(self.clock.to_json()),
        }
    }
}

/// `text`, cut to `max` characters on a character boundary. Every string a
/// caller sends that an attempt stores goes through here.
pub(super) fn capped(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((cut, _)) => format!("{}… (truncated)", &text[..cut]),
        None => text.to_string(),
    }
}

/// Keep the newest [`KEEP_ATTEMPTS`] of the app's attempts with `outcome`.
/// Best effort: a prune that fails leaves the next attempt to try again.
async fn prune(db: &DatabaseConnection, app_id: Uuid, outcome: &str) {
    let keep: Result<Vec<Uuid>, _> = app_publish_attempts::Entity::find()
        .select_only()
        .column(app_publish_attempts::Column::Id)
        .filter(app_publish_attempts::Column::AppId.eq(app_id))
        .filter(app_publish_attempts::Column::Outcome.eq(outcome))
        .order_by_desc(app_publish_attempts::Column::StartedAt)
        .limit(KEEP_ATTEMPTS)
        .into_tuple()
        .all(db)
        .await;
    let keep = match keep {
        Ok(keep) if (keep.len() as u64) < KEEP_ATTEMPTS => return,
        Ok(keep) => keep,
        Err(e) => {
            tracing::warn!(app_id = %app_id, error = %e, "publish attempts not pruned");
            return;
        }
    };
    let pruned = app_publish_attempts::Entity::delete_many()
        .filter(app_publish_attempts::Column::AppId.eq(app_id))
        .filter(app_publish_attempts::Column::Outcome.eq(outcome))
        .filter(app_publish_attempts::Column::Id.is_not_in(keep))
        .exec(db)
        .await;
    if let Err(e) = pruned {
        tracing::warn!(app_id = %app_id, error = %e, "publish attempts not pruned");
    }
}

#[cfg(test)]
#[path = "record_tests.rs"]
mod tests;
