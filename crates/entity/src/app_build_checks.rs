//! One check of one custom-app build: a build × environment × kind × attempt
//! (`oxy_api_custom_apps::custom_apps_build_checks`).
//!
//! Every build that reaches an app's staging is checked: a queued task starts
//! a runner, and each check has its row here, `queued` when the task is
//! queued, `running` once a runner claims the attempt, then `passed`,
//! `failed`, `error` (the check could not reach a verdict) or `skipped`
//! (nothing to check, or nothing to check it with). A rerun is the next
//! `attempt`; earlier attempts stay as history.
//!
//! Platform data about a build, like `app_builds`: statuses, a one-line summary
//! and capped findings (a page error's message, a CSS `file:line`, a check
//! function's error). Never an answer body or page content. Deleted with its
//! build (`ON DELETE CASCADE`).

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "app_build_checks")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    #[sea_orm(indexed)]
    pub app_id: Uuid,
    /// The build checked (`app_builds.id`).
    pub build_id: Uuid,
    /// Where it was checked: `staging` in phase 1.
    pub environment: String,
    /// `validation`, `functions`, `loads`, `design_system`; later phases add
    /// more (`smoke`, `changes`, `design_audit`, `production_smoke`), and the
    /// build's advisory `summary` is a row of its own kind.
    pub kind: String,
    /// 1 for the checks a staging move queued, then one more per rerun.
    pub attempt: i32,
    /// `queued` | `running` | `passed` | `failed` | `error` | `skipped`.
    pub status: String,
    /// Informs only: never part of a build's verdict (a design audit, what
    /// changed, the build's summary).
    pub advisory: bool,
    /// One line: what the check found.
    pub summary: Option<String>,
    /// `{findings: [...], ...}`, the kind's evidence.
    pub detail: Option<Json>,
    /// Advisory notes on the check, each `{author: "rule"|"model", model,
    /// text, refs, created_at}`. Written beside `detail`, never into it, so a
    /// later note cannot overwrite the runner's evidence.
    pub notes: Json,
    /// The queued run that started the attempt's runner (`agentic_runs.id`).
    pub run_id: Option<String>,
    /// Who is running the attempt, the same value on every row of it. One of
    /// three: null (a runner was started and has not claimed the attempt);
    /// a runner, `{label, url, started_at}`, written by each claim with its
    /// own `started_at`; or the record that nothing was started for it,
    /// `{not_started: "not_configured", reason, at}`, written by the task on
    /// a deployment that starts no runner and replaced by a claim.
    pub runner: Option<Json>,
    pub created_at: DateTimeWithTimeZone,
    pub started_at: Option<DateTimeWithTimeZone>,
    pub finished_at: Option<DateTimeWithTimeZone>,
    #[sea_orm(
        belongs_to,
        from = "app_id",
        to = "id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    #[serde(skip)]
    pub apps: BelongsTo<super::apps::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}
