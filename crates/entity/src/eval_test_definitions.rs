//! `eval_test_definitions` — one row per `.test.yml` (an eval suite) parsed in
//! a revision, so `POST /api/{workspace_id}/evals/runs` reads the suite from
//! the compile boundary on the worker fleet rather than from a working copy.
//!
//! Keyed by path, not name: a test file is addressed by where it sits, and its
//! default target is the agent beside it.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "eval_test_definitions")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub revision_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub file_path: String,
    pub definition: Json,
    #[sea_orm(
        belongs_to,
        from = "revision_id",
        to = "revision_id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    #[serde(skip)]
    pub revisions: BelongsTo<super::revisions::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}
