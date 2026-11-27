//! Acknowledge / dismiss / reopen — the writes.
//!
//! Two shapes the rest of the module leans on: a write names *events* rather
//! than the buckets a caller happens to hold (a page caps buckets per event, so
//! enumerating them leaves a chain's tail behind), and it carries the statuses
//! it may touch, so acking a live row cannot reverse a dismissal the user made
//! and can no longer see.

use agentic_runtime::agentic_state::AgenticState;
use axum::extract::{Extension, Json, Path};
use chrono::Utc;
use entity::metric_anomalies::{self, Entity as AnomaliesEntity};
use sea_orm::ActiveValue::Set;
use sea_orm::sea_query::{Expr, ExprTrait};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, QuerySelect,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use super::error::AnomalyError;
use oxy_server_authz::workspace_role::EffectiveWorkspaceRole;
use oxy_workspace_runtime::request::workspace_context::WorkspaceManagerReadOnly;

#[derive(Debug, Deserialize)]
pub struct UpdateStatusRequest {
    pub status: String,
}

#[derive(Debug, Deserialize)]
pub struct BulkUpdateStatusRequest {
    /// Individual anomaly row ids. For rows detected before events existed —
    /// they have no `event_id`, so they can only be named one at a time.
    #[serde(default)]
    pub ids: Vec<Uuid>,
    /// Event ids: every bucket of these events is written, including buckets
    /// the caller never saw.
    ///
    /// This is what makes an Ack whole. The inbox acts on events, and a list
    /// response is capped at [`super::cap::MAX_BUCKETS_PER_EVENT`] buckets per event — so a
    /// client that enumerated the bucket ids it received would silently leave
    /// the rest of a long chain `new` while reporting success. Naming the event
    /// moves the completeness question to the server, which is the only side
    /// that knows the full set.
    #[serde(default)]
    pub event_ids: Vec<Uuid>,
    /// Restrict the write to buckets already in one of these statuses.
    ///
    /// **Absent** takes the safe default rather than "no restriction": the live
    /// statuses, plus `dismissed` when the target is `new`, since reopening is
    /// how a dismissed anomaly comes back. An explicit `[]` is the opt-out. The
    /// endpoint is on the external surface and documented for hand-rolled
    /// callers, and reversing a dismissal by accident is the one outcome this
    /// whole design exists to prevent — so it cannot be what you get by leaving
    /// a field off.
    ///
    /// `event_ids` on its own is too broad in one direction and a plain
    /// "only the status I was viewing" is too narrow in the other, so this is a
    /// set rather than a single value.
    ///
    /// Too broad: an event can hold buckets the caller deliberately dismissed
    /// weeks ago and cannot see from the tab they are in; acking the row should
    /// not resurrect them. Too narrow: a scan chains a fresh `new` bucket onto
    /// an event that was already acknowledged, so a write restricted to
    /// `acknowledged` alone would leave that bucket behind and the event would
    /// stay in the New tab, counted by the badge, under a toast claiming it was
    /// handled.
    ///
    /// The inbox therefore sends the *live* statuses (`new`, `acknowledged`)
    /// and adds `dismissed` only when the row being acted on is itself
    /// dismissed — i.e. when those buckets are the ones on screen.
    #[serde(default)]
    pub only_statuses: Option<Vec<String>>,
    pub status: String,
}

/// Refuse a status scope longer than the enum it draws from, before it is
/// cloned and walked twice. Three distinct values exist, so a fourth entry is a
/// caller sending noise — and discovering it in the dedupe means a 200k-entry
/// body has already been scanned to get there.
///
/// Bounded by the enum rather than by [`MAX_BULK_IDS`]: that constant exists to
/// bound an `IN (...)` list of uuids, and borrowing it here would silently
/// retune this scope whenever that one is retuned for a reason that has nothing
/// to do with statuses.
fn check_scope_length(sent: Option<&Vec<String>>) -> Result<(), AnomalyError> {
    match sent {
        // Named, not folded into `TooManyIds`: a caller that sent 25 ids and a
        // bloated status list would otherwise be told to shrink its id list —
        // the same misdirection `BadRequest` exists to stop.
        // Counted in *entries*, and the message says so. This runs before the
        // dedupe, so four entries drawn from three distinct values is still
        // four — a message that said "at most three values" would be one the
        // refused request appears to satisfy.
        Some(list) if list.len() > 3 => Err(AnomalyError::BadRequest(format!(
            "only_statuses has {} entries; at most three entries, drawn from \
             new | acknowledged | dismissed (duplicates count)",
            list.len()
        ))),
        _ => Ok(()),
    }
}

/// The scope a request gets when it names none — mirrors the SDK's
/// `defaultScope`. Live statuses for an ack or a dismiss; all three for a
/// reopen, which exists to reach dismissed buckets.
fn default_only_statuses(status: &str) -> Vec<String> {
    let mut scope = vec!["new".to_string(), "acknowledged".to_string()];
    if status == "new" {
        scope.push("dismissed".to_string());
    }
    scope
}

#[derive(Debug, Serialize)]
pub struct BulkUpdateStatusResponse {
    /// Rows actually written — buckets, not selections, so an event resolved
    /// from `event_ids` contributes all of its own. Zero means nothing matched
    /// (a stale selection); the caller can tell that from a clean apply rather
    /// than assuming its request landed.
    pub updated: u64,
    /// Distinct **anomalies** behind those rows — events, plus standalone
    /// pre-event rows. The UI counts in this unit, and it is the only side that
    /// can compute it: a caller naming an event never learns how many buckets
    /// that was, so without this a partly-applied batch would be reported as
    /// if all of it landed.
    ///
    /// Caveat for the `ids` arm: an anomaly counts as updated once *any* of its
    /// buckets is written. Name an event through `event_ids` and that is the
    /// whole event, so the count means what it says; name one bucket of a
    /// ten-bucket event through `ids` and this still reports `1` while nine
    /// buckets keep their old status. `ids` exists for pre-event rows, which
    /// are one bucket each and so can't hit this — a caller using it for
    /// anything else is asking for a partial write and gets one.
    pub events_updated: u64,
}

/// Most ids (row + event, combined) one bulk call may carry — identifiers, not
/// the buckets behind them; [`MAX_BULK_ROWS`] is what bounds those. A full-page
/// selection is 25 events, so this is only ever reached by a hand-rolled
/// caller.
pub(super) const MAX_BULK_IDS: usize = 2_000;

/// Most rows one bulk call may write.
///
/// [`MAX_BULK_IDS`] bounds identifiers, not the buckets behind them: 2000 event
/// ids over long chains is a six-figure row set, and that whole set is one
/// `UPDATE` inside one transaction, holding row locks for its duration and
/// blocking the scan's upserts on those segments. The UI cannot approach this
/// (a page is 25 events), so refusing is better than chunking — a caller that
/// hits it is doing something the surface was not built for and should hear so
/// rather than have the work silently split.
pub(super) const MAX_BULK_ROWS: u64 = 20_000;

/// `POST /workspaces/{workspace_id}/semantic/anomalies/status` with body
/// `{"ids": [...], "event_ids": [...], "status": "acknowledged" | … }`.
///
/// Workspace-scoped: the `workspace_id` predicate is the tenant boundary, so an
/// id from another workspace silently updates nothing instead of leaking or
/// writing across tenants.
pub async fn update_status_bulk(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    _role: EffectiveWorkspaceRole,
    Extension(state): Extension<Arc<AgenticState>>,
    Path(_workspace_id): Path<Uuid>,
    Json(req): Json<BulkUpdateStatusRequest>,
) -> Result<Json<BulkUpdateStatusResponse>, AnomalyError> {
    if !matches!(req.status.as_str(), "new" | "acknowledged" | "dismissed") {
        return Err(AnomalyError::BadStatus(req.status));
    }
    check_scope_length(req.only_statuses.as_ref())?;
    let requested_scope = req
        .only_statuses
        .clone()
        .unwrap_or_else(|| default_only_statuses(&req.status));
    if let Some(bad) = requested_scope
        .iter()
        .find(|s| !matches!(s.as_str(), "new" | "acknowledged" | "dismissed"))
    {
        // Names the field, like the length check above it. `BadStatus` would
        // say "invalid status 'open'" about a request whose `status` is fine —
        // sending whoever is debugging to the wrong half of the body.
        return Err(AnomalyError::BadRequest(format!(
            "invalid only_statuses entry '{bad}' (expected: new | acknowledged | dismissed)"
        )));
    }
    // Deduped so the `IN (...)` carries three values at most; the length guard
    // above is what stops an oversized list reaching this at all.
    let only_statuses = {
        let mut seen = std::collections::HashSet::with_capacity(3);
        requested_scope
            .iter()
            .filter(|s| seen.insert(s.as_str()))
            .cloned()
            .collect::<Vec<_>>()
    };
    // Capped on the two lists together — either one alone is a way to send an
    // unbounded `IN (...)`.
    let (ids, event_ids) = normalize_bulk_ids(req.ids, req.event_ids)?;
    let written = apply_status_bulk(
        &state.db,
        workspace_manager.workspace_id,
        &ids,
        &event_ids,
        &only_statuses,
        &req.status,
    )
    .await?;
    Ok(Json(BulkUpdateStatusResponse {
        updated: written.rows,
        events_updated: written.events,
    }))
}

/// Enforce the combined cap, then deduplicate each list.
///
/// The dedupe keeps the `IN (...)` lists minimal; the cap is checked against
/// what the caller *sent*, not what survives dedupe, so a 10k-entry body is
/// refused rather than quietly collapsed.
pub(crate) fn normalize_bulk_ids(
    ids: Vec<Uuid>,
    event_ids: Vec<Uuid>,
) -> Result<(Vec<Uuid>, Vec<Uuid>), AnomalyError> {
    let total = ids.len().saturating_add(event_ids.len());
    if total > MAX_BULK_IDS {
        return Err(AnomalyError::TooManyIds(total));
    }
    Ok((dedupe(ids), dedupe(event_ids)))
}

fn dedupe(ids: Vec<Uuid>) -> Vec<Uuid> {
    let mut seen = std::collections::HashSet::with_capacity(ids.len());
    ids.into_iter().filter(|id| seen.insert(*id)).collect()
}

/// Set `status` on every row named by `ids`, plus every bucket of every event
/// named by `event_ids`, within `workspace_id`. Returns what was written in
/// both units: buckets, and the distinct anomalies they belong to — the second
/// is the whole reason the count statement below exists.
///
/// Resolving events server-side is the point: a list response caps buckets per
/// event, so a caller that enumerated what it received could leave the tail of
/// a long chain behind. Naming the event writes all of it.
///
/// Two statements, not one: the anomaly count has to be read before the write,
/// because afterwards the rows no longer carry the status the write was bounded
/// to.
///
/// **And no transaction around them**, which is a decision rather than an
/// omission. The read runs first and returns on failure before any `UPDATE` is
/// issued, and the write is a single atomic statement — so there is no partial
/// state a wrapper could roll back. Nor would it buy a consistent view: under
/// READ COMMITTED each statement takes its own snapshot either way, so a bucket
/// a scan commits between the two is invisible to the count and visible to the
/// `UPDATE`: `rows` can exceed what `events` describes, and the toast
/// under-reports. Without the wrapper that window also spans a pool
/// acquisition, since the two statements may land on different connections —
/// wider than a `BEGIN` made it, and still bounded by the next refetch, which
/// corrects the table. That is why this is left as a counting skew rather than
/// paid for with REPEATABLE READ — that would turn two people acking the same
/// event into a serialization error, trading a cosmetic mismatch for a real
/// one. A `BEGIN`/`COMMIT` pair and a connection pinned across both statements
/// buy neither, so they are not spent.
///
/// `only_statuses` narrows that reach to the buckets the caller meant. An event
/// can span statuses: without a bound, acking a row in the **New** tab would
/// also drag that event's deliberately-dismissed buckets along; bound to the
/// single status they were viewing, a `new` bucket a scan chained onto an
/// acknowledged event would be left behind instead. Empty means no bound.
///
/// The `workspace_id` predicate is the tenant boundary — an id from another
/// workspace matches nothing here rather than being written or leaked.
pub async fn apply_status_bulk(
    db: &sea_orm::DatabaseConnection,
    workspace_id: Uuid,
    ids: &[Uuid],
    event_ids: &[Uuid],
    only_statuses: &[String],
    status: &str,
) -> Result<StatusWriteCounts, AnomalyError> {
    apply_status_bulk_capped(
        db,
        workspace_id,
        ids,
        event_ids,
        only_statuses,
        status,
        MAX_BULK_ROWS,
    )
    .await
}

/// [`apply_status_bulk`] with the row ceiling as a parameter, so a test can
/// drive the refusal without seeding twenty thousand buckets to reach it.
pub async fn apply_status_bulk_capped(
    db: &sea_orm::DatabaseConnection,
    workspace_id: Uuid,
    ids: &[Uuid],
    event_ids: &[Uuid],
    only_statuses: &[String],
    status: &str,
    max_rows: u64,
) -> Result<StatusWriteCounts, AnomalyError> {
    // An empty selection is a no-op, not an error — and `IN ()` is not valid SQL.
    if ids.is_empty() && event_ids.is_empty() {
        return Ok(StatusWriteCounts::default());
    }
    // `..Default::default()` leaves every other column `NotSet`, so the
    // statement writes exactly these two.
    let update = metric_anomalies::ActiveModel {
        status: Set(status.to_string()),
        updated_at: Set(Utc::now().into()),
        ..Default::default()
    };
    // Empty lists are left out of the disjunction entirely: `IN ()` is invalid,
    // and an always-false arm would be noise in the plan.
    let mut targets = sea_orm::Condition::any();
    if !ids.is_empty() {
        targets = targets.add(metric_anomalies::Column::Id.is_in(ids.to_vec()));
    }
    if !event_ids.is_empty() {
        targets = targets.add(metric_anomalies::Column::EventId.is_in(event_ids.to_vec()));
    }
    // Read first, then write, on the pool — no transaction. See the doc block:
    // the write is one atomic statement with nothing to roll back, and READ
    // COMMITTED gives each statement its own snapshot with or without a
    // wrapper, so the counting skew a `BEGIN` looks like it would close is
    // there either way.

    // Counted in SQL, not in the handler. The anomaly count cannot come from
    // `rows_affected`, but neither of the ways to get it client-side is bounded:
    // `RETURNING *` materialises every written `Model` (`explain_cache` and
    // `filters` JSONB included), and a narrow projection still pulls one tuple
    // per bucket — `MAX_BULK_IDS` bounds identifiers, not the buckets behind
    // them, so 2000 event ids over long chains is a six-figure row set either
    // way. `COUNT(DISTINCT …)` returns the same number and matches the shape
    // the index already covers.
    let mut candidates = AnomaliesEntity::find()
        .filter(metric_anomalies::Column::WorkspaceId.eq(workspace_id))
        .filter(targets.clone())
        // Rows already in the target status are not part of this write.
        // Postgres reports matched rows, not changed ones, so without this a
        // no-op — "Ack selected" over an already-acknowledged selection —
        // would come back as a full success and rewrite `updated_at` on every
        // untouched row.
        .filter(metric_anomalies::Column::Status.ne(status));
    if !only_statuses.is_empty() {
        candidates =
            candidates.filter(metric_anomalies::Column::Status.is_in(only_statuses.to_vec()));
    }
    let (events, candidate_rows) = candidates
        .select_only()
        .column_as(
            Expr::col(metric_anomalies::Column::EventId)
                .if_null(Expr::col(metric_anomalies::Column::Id))
                .count_distinct(),
            "events",
        )
        .column_as(Expr::col(metric_anomalies::Column::Id).count(), "rows")
        .into_tuple::<(i64, i64)>()
        .one(db)
        .await?
        .unwrap_or((0, 0));
    let events = Ord::max(events, 0) as u64;
    let candidate_rows = Ord::max(candidate_rows, 0) as u64;
    // Refused before the `UPDATE`, so an oversized selection costs one indexed
    // count rather than a six-figure write holding row locks for its duration.
    // Nothing has been written yet at this point, so returning here leaves the
    // table exactly as it was.
    if candidate_rows > max_rows {
        return Err(AnomalyError::TooManyRows {
            rows: candidate_rows,
            limit: max_rows,
        });
    }

    let mut query = AnomaliesEntity::update_many()
        .set(update)
        .filter(metric_anomalies::Column::WorkspaceId.eq(workspace_id))
        .filter(targets)
        // Same exclusion as the count above, so the two agree and a no-op
        // reports as one.
        .filter(metric_anomalies::Column::Status.ne(status));
    if !only_statuses.is_empty() {
        query = query.filter(metric_anomalies::Column::Status.is_in(only_statuses.to_vec()));
    }
    let rows = query.exec(db).await?.rows_affected;
    Ok(StatusWriteCounts { rows, events })
}

/// What a status write actually changed, in both units the UI needs.
#[derive(Debug, Default, Clone, Copy)]
pub struct StatusWriteCounts {
    /// Buckets written.
    pub rows: u64,
    /// Distinct anomalies those buckets belong to.
    pub events: u64,
}

/// `POST /workspaces/{workspace_id}/semantic/anomalies/{id}/status`
/// with body `{"status": "acknowledged" | "dismissed" | "new"}`.
pub async fn update_status(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    _role: EffectiveWorkspaceRole,
    Extension(state): Extension<Arc<AgenticState>>,
    Path((_workspace_id, anomaly_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<UpdateStatusRequest>,
) -> Result<Json<metric_anomalies::Model>, AnomalyError> {
    if !matches!(req.status.as_str(), "new" | "acknowledged" | "dismissed") {
        return Err(AnomalyError::BadStatus(req.status));
    }
    let workspace_id = workspace_manager.workspace_id;
    let existing = AnomaliesEntity::find_by_id(anomaly_id)
        .filter(metric_anomalies::Column::WorkspaceId.eq(workspace_id))
        .one(&state.db)
        .await?
        .ok_or(AnomalyError::NotFound)?;
    let mut active = existing.into_active_model();
    active.status = Set(req.status);
    active.updated_at = Set(Utc::now().into());
    let updated = active.update(&state.db).await?;
    Ok(Json(updated))
}

#[cfg(test)]
mod tests {
    use super::{AnomalyError, MAX_BULK_IDS, check_scope_length, normalize_bulk_ids};
    use uuid::Uuid;

    #[test]
    fn bulk_ids_are_deduped_in_the_order_sent() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let (ids, event_ids) = normalize_bulk_ids(vec![a, b, a], vec![b, b]).unwrap();
        assert_eq!(ids, vec![a, b]);
        assert_eq!(event_ids, vec![b]);
        let (ids, event_ids) = normalize_bulk_ids(vec![], vec![]).unwrap();
        assert!(ids.is_empty() && event_ids.is_empty());
    }

    #[test]
    fn a_status_scope_longer_than_the_enum_is_refused() {
        // Three distinct values exist, so a fourth entry is noise — and it must
        // be refused before the clone and the two O(n) passes that would
        // otherwise walk it first. Duplicates count: the dedupe that would
        // collapse them runs after this.
        let noise = vec!["new".to_string(); 4];
        assert!(matches!(
            check_scope_length(Some(&noise)),
            Err(AnomalyError::BadRequest(_))
        ));
        assert!(check_scope_length(Some(&vec!["new".to_string(); 3])).is_ok());
        assert!(check_scope_length(None).is_ok());
    }

    #[test]
    fn bulk_ids_are_capped_on_what_the_caller_sent() {
        let over: Vec<Uuid> = (0..=MAX_BULK_IDS as u128).map(Uuid::from_u128).collect();
        assert!(matches!(
            normalize_bulk_ids(over, vec![]),
            Err(AnomalyError::TooManyIds(_))
        ));
        // …on the raw lists, not the deduped ones: a 10k-entry body of one
        // repeated id is still a 10k-entry body.
        let dupes = vec![Uuid::from_u128(7); MAX_BULK_IDS + 1];
        assert!(matches!(
            normalize_bulk_ids(dupes, vec![]),
            Err(AnomalyError::TooManyIds(_))
        ));
        // …and on the two lists *together*, so neither one alone is a way past
        // the cap.
        let half: Vec<Uuid> = (0..MAX_BULK_IDS as u128 / 2 + 1)
            .map(Uuid::from_u128)
            .collect();
        assert!(matches!(
            normalize_bulk_ids(half.clone(), half),
            Err(AnomalyError::TooManyIds(_))
        ));
    }
}
