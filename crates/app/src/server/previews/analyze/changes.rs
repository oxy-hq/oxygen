//! Which `.airway.yml` files a branch changed: the staging revision's compiled
//! `airway_pipelines` rows against the promoted revision's, matched by file
//! path. Postgres only — the compile boundary already holds both sides.

use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, FromQueryResult, Statement};
use uuid::Uuid;

/// One pipeline file the branch added, edited or removed. `branch_def` is the
/// staging revision's compiled definition (`None`: removed); `live_def` the
/// promoted revision's (`None`: added).
#[derive(Clone, Debug, PartialEq, FromQueryResult)]
pub struct ChangedPipeline {
    /// The branch's `name` for an added or edited file, the live one for a
    /// removed file.
    pub name: String,
    pub file_path: String,
    pub branch_def: Option<serde_json::Value>,
    pub live_def: Option<serde_json::Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Added,
    Modified,
    Removed,
}

impl Change {
    pub fn as_str(self) -> &'static str {
        match self {
            Change::Added => "added",
            Change::Modified => "modified",
            Change::Removed => "removed",
        }
    }
}

impl ChangedPipeline {
    pub fn change(&self) -> Change {
        match (&self.live_def, &self.branch_def) {
            (None, _) => Change::Added,
            (Some(_), None) => Change::Removed,
            (Some(_), Some(_)) => Change::Modified,
        }
    }
}

/// Edits and additions (a staging file with no identical promoted file at the
/// same path), then removals (a promoted file the staging revision lacks),
/// ordered by path. A file whose compiled definition is byte-for-byte the same
/// on both sides is not a change. With no promoted revision every staging
/// pipeline is an addition.
const CHANGED_SQL: &str = r#"
SELECT name, file_path, branch_def, live_def FROM (
    SELECT s.name, s.file_path, s.definition AS branch_def, m.definition AS live_def
    FROM airway_pipelines s
    LEFT JOIN airway_pipelines m ON m.revision_id = $2 AND m.file_path = s.file_path
    WHERE s.revision_id = $1
      AND (m.file_path IS NULL OR m.definition IS DISTINCT FROM s.definition)
    UNION ALL
    SELECT m.name, m.file_path, NULL AS branch_def, m.definition AS live_def
    FROM airway_pipelines m
    WHERE m.revision_id = $2
      AND NOT EXISTS (SELECT 1 FROM airway_pipelines s
                      WHERE s.revision_id = $1 AND s.file_path = m.file_path)
) changed
ORDER BY file_path, name
"#;

/// What `staging` changed against `promoted`.
pub async fn changed_pipelines<C: ConnectionTrait>(
    db: &C,
    staging: Uuid,
    promoted: Option<Uuid>,
) -> Result<Vec<ChangedPipeline>, DbErr> {
    ChangedPipeline::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        CHANGED_SQL,
        [staging.into(), promoted.into()],
    ))
    .all(db)
    .await
}
