//! What a sandbox's own OLTP schema is, as its row records it
//! (`app_environments.oltp_schema`), and what that makes of the sandbox's
//! `ctx.oltp` (`internal-docs/per-org-oltp-postgres.md` → Sandbox schemas on
//! the staging branch).
//!
//! The schema lives in the org's staging branch database, where only a queued
//! task creates it and a reset of the branch destroys it. Two readers must
//! know its state without connecting there: a function's admission
//! ([`home_for`]) and the environment view ([`OltpSchemaDto`]). So the task
//! writes the state here — `seeding` before the schema exists, then `ready`
//! or `failed` — together with **the branch cut it was seeded on**. A cut
//! that is no longer the branch's is a reset: the schema is gone, whatever
//! the status says.

use chrono::{DateTime, FixedOffset, Utc};
use entity::app_environments;
use oxy_app_core::custom_app_environment::{AppEnvironment, AppEnvironmentKind};
use oxy_oltp::branches::BranchCut;
use oxy_oltp::sandbox_schema::SandboxSchema;
use oxy_oltp::sandbox_schema::seed::SeedReport;
use sea_orm::{ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, QuerySelect};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::server::api::custom_apps_functions::env_policy::{
    OltpHome, SandboxHome, SandboxUnready,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OltpSchemaStatus {
    /// The task is creating and seeding it. Written before the schema exists,
    /// so a schema is never on the branch with no row recording it.
    Seeding,
    Ready,
    /// The create or the seed failed; `error` says why. The next publish
    /// starts again from nothing.
    Failed,
}

/// The `app_environments.oltp_schema` document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OltpSchemaState {
    pub schema: String,
    pub status: OltpSchemaStatus,
    /// The branch cut the schema was created on: the `oltp_branches` row, the
    /// provider's branch id, and when the branch's data was copied.
    pub branch_row_id: Uuid,
    pub branch: String,
    pub cut_at: DateTime<FixedOffset>,
    /// When the task marked it `seeding`. A seed whose worker died never
    /// writes `ready` or `failed`; past [`SEED_STALLED_AFTER`] the view
    /// reports it `stale`.
    #[serde(default)]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub seeded_at: Option<DateTime<Utc>>,
    /// How many of staging's tables were copied.
    #[serde(default)]
    pub tables: usize,
    /// The tables copied empty: over the seed's size cap.
    #[serde(default)]
    pub structure_only: Vec<String>,
    /// What the copies still use of staging's schema (a type, a function) —
    /// what stops a staging migration dropping it while the sandbox exists.
    #[serde(default)]
    pub staging_dependencies: Vec<String>,
    #[serde(default)]
    pub error: Option<String>,
}

/// Why a `stale` schema is stale: the branch it was seeded on was reset, or
/// re-cut, or removed — which took the schema with it.
pub const BRANCH_RESET: &str =
    "the org's OLTP staging branch was reset (or removed) since this schema was seeded";

/// Why a `stale` schema is stale: its seed never finished.
pub const SEED_STALLED: &str = "the seed did not finish (its worker may have stopped)";

/// How long a seed may sit `seeding` before the view calls it stalled: the
/// task's deadline (5 minutes) and its lock and `Busy` waits, with room.
pub const SEED_STALLED_AFTER: chrono::Duration = chrono::Duration::minutes(15);

impl OltpSchemaState {
    /// A schema about to be created on `cut`.
    pub fn seeding(schema: &SandboxSchema, cut: &BranchCut) -> Self {
        Self {
            schema: schema.name().to_string(),
            status: OltpSchemaStatus::Seeding,
            branch_row_id: cut.row_id,
            branch: cut.provider_branch_id.clone(),
            cut_at: cut.cut_at,
            started_at: Some(Utc::now()),
            seeded_at: None,
            tables: 0,
            structure_only: Vec::new(),
            staging_dependencies: Vec::new(),
            error: None,
        }
    }

    pub fn ready(mut self, report: &SeedReport) -> Self {
        self.status = OltpSchemaStatus::Ready;
        self.seeded_at = Some(Utc::now());
        self.tables = report.tables.len();
        self.structure_only = report.structure_only.clone();
        self.staging_dependencies = report.staging_dependencies.clone();
        self.error = None;
        self
    }

    pub fn failed(mut self, error: String) -> Self {
        self.status = OltpSchemaStatus::Failed;
        self.error = Some(error);
        self
    }

    /// Whether this is `schema`, on the branch as it is now cut.
    pub fn is_on(&self, schema: &SandboxSchema, cut: &BranchCut) -> bool {
        self.schema == schema.name()
            && self.branch_row_id == cut.row_id
            && self.branch == cut.provider_branch_id
            && self.cut_at == cut.cut_at
    }

    /// Whether the schema is seeded and still on the branch's current cut.
    pub fn is_ready_on(&self, schema: &SandboxSchema, cut: &BranchCut) -> bool {
        self.status == OltpSchemaStatus::Ready && self.is_on(schema, cut)
    }
}

/// Where a sandbox's `ctx.oltp` lands in an org whose staging branch is cut
/// as `cut`: its own schema when the row records it ready on that cut;
/// refused, with the reason, in every other state. Never staging's schema.
pub fn home_for(
    state: Option<&OltpSchemaState>,
    schema: &SandboxSchema,
    cut: &BranchCut,
) -> OltpHome {
    let unready = match state {
        None => SandboxUnready::NotCreated,
        Some(state) if !state.is_on(schema, cut) => SandboxUnready::BranchReset,
        Some(state) => match state.status {
            OltpSchemaStatus::Ready => {
                return OltpHome::SandboxSchema(SandboxHome {
                    cut: cut.clone(),
                    schema: schema.clone(),
                });
            }
            OltpSchemaStatus::Seeding => SandboxUnready::Seeding,
            OltpSchemaStatus::Failed => SandboxUnready::Failed,
        },
    };
    OltpHome::SandboxUnready(unready)
}

/// The state of `environment`'s row. `None` for a row that records none, a
/// row that is gone, and a document this build cannot read (a later build's):
/// each is "no usable schema", which a publish repairs.
pub async fn read<C: ConnectionTrait>(
    db: &C,
    app_id: Uuid,
    environment: &AppEnvironment,
) -> Result<Option<OltpSchemaState>, DbErr> {
    let stored: Option<Option<serde_json::Value>> = app_environments::Entity::find()
        .select_only()
        .column(app_environments::Column::OltpSchema)
        .filter(app_environments::Column::AppId.eq(app_id))
        .filter(app_environments::Column::Name.eq(environment.name()))
        .filter(app_environments::Column::Kind.eq(AppEnvironmentKind::Dev.as_str()))
        .into_tuple()
        .one(db)
        .await?;
    Ok(stored.flatten().and_then(parse))
}

/// Whether `environment`'s row records a schema at all — whether or not this
/// build can read the document. The teardown asks this, not [`read`]: a state
/// written by a newer build must still get its schema dropped.
pub async fn is_recorded<C: ConnectionTrait>(
    db: &C,
    app_id: Uuid,
    environment: &AppEnvironment,
) -> Result<bool, DbErr> {
    let stored: Option<Option<serde_json::Value>> = app_environments::Entity::find()
        .select_only()
        .column(app_environments::Column::OltpSchema)
        .filter(app_environments::Column::AppId.eq(app_id))
        .filter(app_environments::Column::Name.eq(environment.name()))
        .filter(app_environments::Column::Kind.eq(AppEnvironmentKind::Dev.as_str()))
        .into_tuple()
        .one(db)
        .await?;
    Ok(stored.flatten().is_some_and(|document| !document.is_null()))
}

/// The state a row holds, when it holds one this build can read.
pub fn of_row(row: &app_environments::Model) -> Option<OltpSchemaState> {
    row.oltp_schema.clone().and_then(parse)
}

fn parse(document: serde_json::Value) -> Option<OltpSchemaState> {
    match serde_json::from_value(document) {
        Ok(state) => Some(state),
        Err(e) => {
            tracing::warn!("app_environments.oltp_schema is not a state this build reads: {e}");
            None
        }
    }
}

/// Record `state` on the sandbox's row — only a row that is not being
/// deleted, so a task that outlived its sandbox writes nothing. `Ok(false)`:
/// there was no such row.
pub async fn write<C: ConnectionTrait>(
    db: &C,
    app_id: Uuid,
    environment: &AppEnvironment,
    state: &OltpSchemaState,
) -> Result<bool, DbErr> {
    use sea_orm::sea_query::Expr;
    let document = serde_json::to_value(state).map_err(|e| DbErr::Custom(e.to_string()))?;
    let updated = app_environments::Entity::update_many()
        .col_expr(app_environments::Column::OltpSchema, Expr::value(document))
        .filter(app_environments::Column::AppId.eq(app_id))
        .filter(app_environments::Column::Name.eq(environment.name()))
        .filter(app_environments::Column::Kind.eq(AppEnvironmentKind::Dev.as_str()))
        .filter(app_environments::Column::DeletingAt.is_null())
        .exec(db)
        .await?;
    Ok(updated.rows_affected > 0)
}

/// A sandbox's OLTP schema, as `GET …/environments` shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OltpSchemaDto {
    pub schema: String,
    /// `seeding` | `ready` | `failed` | `stale` — `stale` when the org's
    /// staging branch was reset (or removed) since the schema was seeded, so
    /// the schema is gone until the next publish seeds it again.
    pub status: &'static str,
    pub seeded_at: Option<DateTime<Utc>>,
    pub tables: usize,
    pub structure_only: Vec<String>,
    /// What the copy still uses of staging's schema; while the sandbox exists
    /// a staging migration cannot drop these.
    pub staging_dependencies: Vec<String>,
    pub error: Option<String>,
}

impl OltpSchemaDto {
    /// `state` as shown while the org's staging branch is cut as `cut`
    /// (`None`: it has no active branch).
    pub fn of(state: OltpSchemaState, cut: Option<&BranchCut>) -> Self {
        let on_this_cut = cut.is_some_and(|cut| {
            state.branch_row_id == cut.row_id
                && state.branch == cut.provider_branch_id
                && state.cut_at == cut.cut_at
        });
        let stalled = state.status == OltpSchemaStatus::Seeding
            && state
                .started_at
                .is_none_or(|started| Utc::now() - started > SEED_STALLED_AFTER);
        // A `stale` error says why, and only why: the client adds what to do.
        let (status, error) = match state.status {
            _ if !on_this_cut => ("stale", Some(BRANCH_RESET.to_string())),
            _ if stalled => ("stale", Some(SEED_STALLED.to_string())),
            OltpSchemaStatus::Seeding => ("seeding", state.error),
            OltpSchemaStatus::Ready => ("ready", state.error),
            OltpSchemaStatus::Failed => ("failed", state.error),
        };
        Self {
            schema: state.schema,
            status,
            seeded_at: state.seeded_at,
            tables: state.tables,
            structure_only: state.structure_only,
            staging_dependencies: state.staging_dependencies,
            error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxy_oltp::WriterRef;

    fn schema() -> SandboxSchema {
        SandboxSchema::for_writer(&WriterRef::app("store").expect("a writer"), "dev_a1")
            .expect("a name")
    }

    fn cut(id: &str, minute: u32) -> BranchCut {
        use chrono::TimeZone;
        BranchCut {
            row_id: Uuid::from_u128(1),
            provider_branch_id: id.to_string(),
            cut_at: Utc
                .with_ymd_and_hms(2026, 10, 2, 9, minute, 0)
                .unwrap()
                .into(),
        }
    }

    fn report(tables: usize, empty: &[&str]) -> SeedReport {
        SeedReport {
            tables: (0..tables).map(|i| format!("t{i}")).collect(),
            structure_only: empty.iter().map(|t| t.to_string()).collect(),
            staging_dependencies: vec!["column orders.status uses type app_store.status".into()],
            ..SeedReport::default()
        }
    }

    /// A seed whose worker died leaves `seeding` forever; past the stall
    /// line the view says so, and what to do. One in progress is `seeding`.
    #[test]
    fn a_seed_that_never_finished_reads_stale_with_what_to_do() {
        let now = cut("br-1", 0);
        let mut seeding = OltpSchemaState::seeding(&schema(), &now);
        assert_eq!(
            OltpSchemaDto::of(seeding.clone(), Some(&now)).status,
            "seeding"
        );
        seeding.started_at = Some(Utc::now() - SEED_STALLED_AFTER - chrono::Duration::minutes(1));
        let shown = OltpSchemaDto::of(seeding.clone(), Some(&now));
        assert_eq!(shown.status, "stale");
        assert_eq!(
            shown.error.as_deref(),
            Some(SEED_STALLED),
            "why, not what to do"
        );
        seeding.started_at = None;
        assert_eq!(OltpSchemaDto::of(seeding, Some(&now)).status, "stale");
    }

    /// What the copy still uses of staging's schema rides from the seed's
    /// report to the view.
    #[test]
    fn the_view_shows_what_the_copy_still_uses_of_stagings_schema() {
        let now = cut("br-1", 0);
        let state = OltpSchemaState::seeding(&schema(), &now).ready(&report(1, &[]));
        let shown = OltpSchemaDto::of(state, Some(&now));
        assert_eq!(
            shown.staging_dependencies,
            ["column orders.status uses type app_store.status"]
        );
    }

    fn unready(home: OltpHome) -> SandboxUnready {
        match home {
            OltpHome::SandboxUnready(why) => why,
            other => panic!("expected an unready sandbox, got {other:?}"),
        }
    }

    #[test]
    fn a_schema_ready_on_the_branchs_cut_is_the_sandboxs_home() {
        let now = cut("br-1", 0);
        let state = OltpSchemaState::seeding(&schema(), &now).ready(&report(3, &["big"]));
        match home_for(Some(&state), &schema(), &now) {
            OltpHome::SandboxSchema(home) => {
                assert_eq!(home.schema, schema());
                assert_eq!(home.cut, now);
            }
            other => panic!("expected the sandbox's schema, got {other:?}"),
        }
    }

    /// Every state but ready-on-this-cut is refused, each with its own reason
    /// — and none of them is staging's schema.
    #[test]
    fn every_other_state_is_refused_with_its_reason() {
        let now = cut("br-1", 0);
        let seeding = OltpSchemaState::seeding(&schema(), &now);
        assert_eq!(
            unready(home_for(None, &schema(), &now)),
            SandboxUnready::NotCreated
        );
        assert_eq!(
            unready(home_for(Some(&seeding), &schema(), &now)),
            SandboxUnready::Seeding
        );
        let failed = seeding.clone().failed("no".into());
        assert_eq!(
            unready(home_for(Some(&failed), &schema(), &now)),
            SandboxUnready::Failed
        );
    }

    /// A reset keeps the branch's id on both providers and moves when its
    /// data was cut; a re-cut changes the id. Either way the schema seeded
    /// before it is gone.
    #[test]
    fn a_schema_seeded_before_a_reset_or_a_recut_is_not_the_home() {
        let seeded_on = cut("br-1", 0);
        let ready = OltpSchemaState::seeding(&schema(), &seeded_on).ready(&report(1, &[]));
        for after in [cut("br-1", 5), cut("br-2", 0)] {
            assert_eq!(
                unready(home_for(Some(&ready), &schema(), &after)),
                SandboxUnready::BranchReset,
                "{after:?}"
            );
            assert!(!ready.is_ready_on(&schema(), &after));
            assert_eq!(
                OltpSchemaDto::of(ready.clone(), Some(&after)).status,
                "stale"
            );
            // A seed that failed before the reset: the reset is the reason now.
            let failed = OltpSchemaState::seeding(&schema(), &seeded_on).failed("no role".into());
            let shown = OltpSchemaDto::of(failed, Some(&after));
            assert_eq!(
                (shown.status, shown.error.as_deref()),
                ("stale", Some(BRANCH_RESET))
            );
        }
        assert_eq!(OltpSchemaDto::of(ready.clone(), None).status, "stale");
        assert_eq!(OltpSchemaDto::of(ready, Some(&seeded_on)).status, "ready");
    }

    /// The document round-trips, and one written by a later build — a status
    /// this build does not know — reads as no state rather than failing the
    /// request that read it.
    #[test]
    fn the_document_round_trips_and_an_unknown_one_reads_as_none() {
        let state = OltpSchemaState::seeding(&schema(), &cut("br-1", 0)).ready(&report(2, &[]));
        let document = serde_json::to_value(&state).expect("serialize");
        assert_eq!(document["status"], "ready");
        assert_eq!(document["schema"], "app_store__dev_a1");
        assert_eq!(parse(document.clone()), Some(state));
        let mut later = document;
        later["status"] = "archived".into();
        assert_eq!(parse(later), None);
    }
}
