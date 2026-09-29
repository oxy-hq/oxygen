//! The pre-aggregation tick a promote owes.
//!
//! `compile_worker` already reconciles the workspace's `preagg_cycle` schedule
//! on every promoted compile, but a reconcile only ever settles the *cadence*.
//! It deliberately leaves `next_run_at` where it was unless the cron
//! expression moved, so the one promote that has new work — an edit that
//! changes what a rollup IS — produces a hash with no built artifact and then
//! waits out the interval. With no `heartbeat:` configured that is ten
//! minutes; at the configured ceiling it is a day. Queries stay on a live
//! scan the whole time and the rollup reads "Not built", which is
//! indistinguishable from pre-aggregation being broken.
//!
//! What this compares is the **revision now current** against the one that was
//! current before the compile, both read out of `semantic_views` — not the
//! declared set against what is built. "Built" lives in the fleet-wide
//! `manifest.json` plus a node-local ledger, and a compile runs on the ide
//! node while the cycle drains wherever the global fleet puts it, so a
//! manifest read here would be answering about the wrong node's disk. Two
//! revisions in Postgres are the same fact from every node, and they answer
//! the narrower question that is actually being asked: *did this promote
//! change a rollup's identity?*
//!
//! Additions only. A rollup whose declaration was DELETED has no replacement
//! coming and cannot be rebuilt — its leftovers need a retraction, which is
//! `preagg_retract`'s job and not something a cycle tick reaches.
//!
//! **The tick's oracle is the builder's source.** This reads the promoted
//! revision out of Postgres, and the cycle it enqueues resolves its views
//! through `preagg_executor::load_views` → `resolve_query_scan_source` →
//! `scan_dir` on a `WorkspaceManager` that
//! `preagg_workspace::build_workspace_manager` pins to that same promoted
//! revision — so a hash detected here is the hash the cycle builds, on every
//! node. The one gap: a cycle claimed before this promote landed may already
//! hold the previous revision, which is why the enqueue joins only a *queued*
//! cycle, never a claimed one.

use std::collections::BTreeSet;

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::Value;
use uuid::Uuid;

/// The revision a workspace is currently serving, or `None` when it has never
/// promoted one. Read before a compile to capture the baseline, and again
/// after to see what the promote moved to.
pub async fn promoted_revision(
    db: &DatabaseConnection,
    workspace_id: Uuid,
) -> Result<Option<Uuid>, sea_orm::DbErr> {
    Ok(entity::workspaces::Entity::find_by_id(workspace_id)
        .one(db)
        .await?
        .and_then(|w| w.current_revision_id))
}

/// Enqueue a pre-aggregation cycle when the promote that just landed changed
/// what a rollup is.
///
/// `baseline` is the revision the workspace was serving *before* the compile.
/// `enabled` is the opt-in the reconcile just resolved from the promoted
/// `config.yml`: a workspace with no `pre_aggregations:` block, or one that
/// set `refresh_worker.enabled: false`, has no cycle to tick and must not get
/// one through this door.
///
/// Best-effort in every direction. The promote has already landed by the time
/// this runs, so nothing here may fail a compile — a read failure or a failed
/// enqueue is logged and the workspace falls back to its heartbeat, which is
/// exactly the behaviour this replaces.
pub async fn nudge_after_promote(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    baseline: Option<Uuid>,
    enabled: bool,
) {
    if !enabled {
        return;
    }
    let current = match promoted_revision(db, workspace_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(
                target: "preagg",
                error = %e,
                %workspace_id,
                "could not read the promoted revision; leaving the pre-aggregation cycle on \
                 its cadence"
            );
            return;
        }
    };
    // Nothing moved: a `promote: true` compile that lost the causality race,
    // or one whose revision was withheld. Either way the served definitions
    // are the ones the previous cycle already saw.
    if baseline == Some(current) {
        return;
    }

    let after = match declared_rollup_hashes(db, current).await {
        Ok(h) if h.is_empty() => return,
        Ok(h) => h,
        Err(e) => {
            tracing::warn!(
                target: "preagg",
                error = %e,
                %workspace_id,
                revision = %current,
                "could not read the promoted revision's semantic views; leaving the \
                 pre-aggregation cycle on its cadence"
            );
            return;
        }
    };
    // No baseline is a first promote, so everything it declares is new — which
    // is also the case where waiting out a heartbeat is least defensible.
    let before = match baseline {
        None => BTreeSet::new(),
        Some(prev) => match declared_rollup_hashes(db, prev).await {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!(
                    target: "preagg",
                    error = %e,
                    %workspace_id,
                    revision = %prev,
                    "could not read the previous revision's semantic views; leaving the \
                     pre-aggregation cycle on its cadence"
                );
                return;
            }
        },
    };

    let new_hashes = newly_declared(&before, &after);
    if new_hashes.is_empty() {
        tracing::debug!(
            target: "preagg",
            %workspace_id,
            revision = %current,
            "promote declares no rollup hash the previous revision did not; the cycle stays \
             on its cadence"
        );
        return;
    }

    match agentic_pipeline::scheduler::enqueue_preagg_cycle_for_promote(db, workspace_id).await {
        Ok(Some(run_id)) => tracing::info!(
            target: "preagg",
            %workspace_id,
            revision = %current,
            %run_id,
            rollups = new_hashes.len(),
            hashes = %sample_hashes(&new_hashes),
            "promote changed what a rollup is; enqueued a pre-aggregation cycle instead of \
             waiting for the next heartbeat"
        ),
        Ok(None) => tracing::info!(
            target: "preagg",
            %workspace_id,
            revision = %current,
            rollups = new_hashes.len(),
            hashes = %sample_hashes(&new_hashes),
            "promote changed what a rollup is; a cycle was already queued, so this joined it"
        ),
        Err(e) => tracing::warn!(
            target: "preagg",
            error = %e,
            %workspace_id,
            rollups = new_hashes.len(),
            "could not enqueue the post-promote pre-aggregation cycle; the workspace falls \
             back to its heartbeat"
        ),
    }
}

/// How many hashes a log line names before it stops listing them.
///
/// A first promote of a large semantic model declares every rollup it has, and
/// the whole set in one `info` field is a log entry nobody reads and everybody
/// pays to ship. The count beside it is the number that matters; these are for
/// recognising *which* rollups when there are few enough to care.
const HASH_SAMPLE: usize = 8;

fn sample_hashes(hashes: &[String]) -> String {
    if hashes.len() <= HASH_SAMPLE {
        return hashes.join(",");
    }
    format!(
        "{},… (+{} more)",
        hashes[..HASH_SAMPLE].join(","),
        hashes.len() - HASH_SAMPLE
    )
}

/// Every rollup hash the revision's semantic views declare.
///
/// `resolve_rollups` reads one view and nothing else — no `parent:` chain, no
/// sibling view — so a per-row walk gives exactly the hashes the cycle will
/// resolve, without building an engine or materialising the layer.
async fn declared_rollup_hashes(
    db: &DatabaseConnection,
    revision_id: Uuid,
) -> Result<BTreeSet<String>, sea_orm::DbErr> {
    let rows = entity::semantic_views::Entity::find()
        .filter(entity::semantic_views::Column::RevisionId.eq(revision_id))
        .all(db)
        .await?;
    Ok(rows
        .iter()
        .flat_map(|row| rollup_hashes(&row.file_path, &row.definition))
        .collect())
}

/// The rollup hashes one compiled `.view.yml` declares.
///
/// `definition` is the YAML tree stored as JSON — `compile_named_yaml` parses
/// with `serde_yaml` straight into a `serde_json::Value` — and JSON is YAML,
/// so the round-trip back through `parse_view_yaml` is what keeps oxy's
/// lenience rules on this path: the `data_source` alias, the optional
/// `description`, the defaulted collections. Deserialising into
/// `airlayer::View` directly would skip the shim and read a view the rest of
/// the product accepts as one declaring no rollups at all.
///
/// A view that will not parse contributes nothing. The promote has already
/// happened; a view the compiler let through and the shim rejects is a
/// validation gap, not this tick's business, and the cycle will skip it too.
fn rollup_hashes(file_path: &str, definition: &Value) -> Vec<String> {
    let yaml = match serde_json::to_string(definition) {
        Ok(y) => y,
        Err(e) => {
            tracing::debug!(target: "preagg", error = %e, file_path, "compiled view definition is not serialisable");
            return Vec::new();
        }
    };
    match oxy_airlayer_compat::parse_view_yaml(&yaml) {
        Ok(view) => oxy_airlayer_compat::preagg::resolve_rollups(&view)
            .into_iter()
            .map(|r| r.hash)
            .collect(),
        Err(e) => {
            tracing::debug!(
                target: "preagg",
                error = %e,
                file_path,
                "compiled view does not parse as a semantic view; it declares no rollups here"
            );
            Vec::new()
        }
    }
}

/// Hashes `after` declares that `before` did not, sorted so a log line and a
/// test read the same order twice.
fn newly_declared(before: &BTreeSet<String>, after: &BTreeSet<String>) -> Vec<String> {
    after.difference(before).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The view the observation was made against: a rollup over two measures,
    /// which gains a third.
    fn orders(measures: &[&str]) -> Value {
        json!({
            "name": "orders",
            "datasource": "local",
            "table": "orders.csv",
            "refresh_key": { "every": "6h" },
            "dimensions": [
                { "name": "order_status", "type": "string", "expr": "status" },
                { "name": "order_date", "type": "date", "expr": "created_at" },
            ],
            "measures": [
                { "name": "total_orders", "type": "count" },
                { "name": "total_order_value", "type": "sum", "expr": "amount" },
                { "name": "refunds", "type": "sum", "expr": "refund_amount" },
            ],
            "pre_aggregations": [{
                "name": "orders_by_month",
                "dimensions": ["order_status"],
                "measures": measures,
                "time_dimension": "order_date",
                "granularity": "month",
            }],
        })
    }

    /// The whole premise: adding a measure to a rollup's `measures:` list
    /// gives it a hash nothing has built. Guarding it here rather than
    /// trusting the airlayer doc comment — the enqueue gate is only as good as
    /// this being true, and it was false before `definition_fingerprint`
    /// folded the definition into the hash.
    #[test]
    fn adding_a_measure_moves_the_rollup_hash() {
        let before = rollup_hashes("orders.view.yml", &orders(&["total_orders"]));
        let after = rollup_hashes(
            "orders.view.yml",
            &orders(&["total_orders", "total_order_value"]),
        );

        assert_eq!(before.len(), 1);
        assert_eq!(after.len(), 1);
        assert_ne!(before[0], after[0], "the measure list is inside the hash");
    }

    /// ...and the gate fires on exactly that difference.
    #[test]
    fn a_re_hashed_rollup_is_newly_declared() {
        let before: BTreeSet<String> = rollup_hashes("v", &orders(&["total_orders"]))
            .into_iter()
            .collect();
        let after: BTreeSet<String> =
            rollup_hashes("v", &orders(&["total_orders", "total_order_value"]))
                .into_iter()
                .collect();

        let new = newly_declared(&before, &after);
        assert_eq!(new.len(), 1);
        assert!(after.contains(&new[0]));
    }

    /// A promote that touched anything else must not tick — this is the
    /// steady-state path and it runs on every promoted compile in the fleet.
    #[test]
    fn an_unchanged_rollup_declares_nothing_new() {
        let hashes: BTreeSet<String> = rollup_hashes("v", &orders(&["total_orders"]))
            .into_iter()
            .collect();
        assert!(newly_declared(&hashes, &hashes).is_empty());
    }

    /// A deleted rollup is not new work. Its artifacts need a retraction, and
    /// a cycle cannot rebuild a hash the layer no longer declares.
    #[test]
    fn a_deleted_rollup_is_not_newly_declared() {
        let before: BTreeSet<String> = rollup_hashes("v", &orders(&["total_orders"]))
            .into_iter()
            .collect();
        assert!(newly_declared(&before, &BTreeSet::new()).is_empty());
    }

    /// A first promote has no baseline, so everything it declares is new.
    #[test]
    fn a_first_promote_declares_every_rollup_new() {
        let after: BTreeSet<String> = rollup_hashes("v", &orders(&["total_orders"]))
            .into_iter()
            .collect();
        assert_eq!(newly_declared(&BTreeSet::new(), &after).len(), 1);
    }

    /// A view with no `pre_aggregations:` block declares nothing — the opt-in
    /// shape, and the reason an empty promoted set returns before comparing.
    #[test]
    fn a_view_without_rollups_declares_none() {
        let plain = json!({ "name": "orders", "datasource": "local", "table": "orders.csv" });
        assert!(rollup_hashes("orders.view.yml", &plain).is_empty());
    }

    /// The `data_source` alias is one of the lenience rules the shim owns, and
    /// it is why this goes back through `parse_view_yaml` rather than
    /// deserialising `airlayer::View`: read the strict way, this view fails
    /// and silently reports no rollups.
    #[test]
    fn the_oxy_datasource_alias_still_resolves_rollups() {
        let mut aliased = orders(&["total_orders"]);
        let map = aliased.as_object_mut().expect("object");
        let ds = map.remove("datasource").expect("datasource");
        map.insert("data_source".to_string(), ds);

        assert_eq!(rollup_hashes("orders.view.yml", &aliased).len(), 1);
    }

    /// A definition the shim rejects contributes nothing rather than
    /// poisoning the comparison — which would tick every promote forever.
    #[test]
    fn an_unparseable_definition_contributes_no_hashes() {
        assert!(rollup_hashes("broken.view.yml", &json!({ "table": 7 })).is_empty());
    }
}
