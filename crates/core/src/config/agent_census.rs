//! How many agents each promoted revision compiled: the workspace card's
//! "N agents".
//!
//! The count used to walk the working copy for `*.agent.yml` files, which
//! counted only retired classic agents (every agentic-only workspace read 0)
//! and only on a node holding the files. Agents are compiled rows
//! (`agent_definitions`), so the count is a query, the same on every role.
//!
//! One grouped query for a whole listing, so it takes revision ids rather than
//! a manager: a listing has no request revision to pin, and building a manager
//! per workspace to count rows would cost a config load each.

use std::collections::HashMap;

use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};
use uuid::Uuid;

use super::artifacts::ArtifactError;

/// The number of compiled agents in each of `revision_ids`. A revision that
/// compiled none is absent from the map: the caller reads absence as zero.
pub async fn compiled_agent_counts(
    revision_ids: &[Uuid],
) -> Result<HashMap<Uuid, usize>, ArtifactError> {
    use entity::agent_definitions::{Column, Entity};
    if revision_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(Uuid, i64)> = Entity::find()
        .select_only()
        .column(Column::RevisionId)
        .column_as(Column::Name.count(), "agents")
        .filter(Column::RevisionId.is_in(revision_ids.to_vec()))
        .group_by(Column::RevisionId)
        .into_tuple()
        .all(&super::compiled::conn().await?)
        .await
        .map_err(|e| ArtifactError::Backend(e.to_string()))?;
    Ok(rows
        .into_iter()
        .map(|(revision_id, n)| (revision_id, usize::try_from(n).unwrap_or(0)))
        .collect())
}
