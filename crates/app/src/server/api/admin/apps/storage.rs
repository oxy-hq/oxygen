//! Staff-facing storage surface for custom apps: the fleet view, the per-app
//! browser, and delete.
//!
//! Two questions, deliberately answered by two different data sources:
//!
//! * **"Who is going to surprise us?"** — the fleet view reads
//!   `app_storage_usage`, because ranking every app by size cannot mean walking
//!   every app's S3 prefix on page load.
//! * **"What exactly is in this app, and can I remove it?"** — the browser reads
//!   S3 live through the existing `list()`, because the rollup deliberately holds
//!   no per-object rows, and an operator investigating right now needs the
//!   current truth rather than a number up to a sweep old.
//!
//! All of these are persisted-data / object-store reads with no node-local disk,
//! so they classify **`FleetOk`** — any replica can serve them.

use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use entity::prelude::{AppStorageUsage, AppStorageUsageSamples, Apps, Organizations};
use entity::{app_storage_usage, app_storage_usage_samples, apps, organizations};

use crate::server::api::custom_apps_storage as storage;
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;

// ── DTOs ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUsageRow {
    pub app_id: Uuid,
    pub app_name: String,
    pub app_slug: String,
    pub org_id: Uuid,
    pub org_name: Option<String>,
    pub bytes: i64,
    pub object_count: i64,
    pub untagged_bytes: i64,
    /// Δ bytes over the trailing week. `None` when there isn't a sample old
    /// enough to difference against — which is honest about a new app rather
    /// than reporting its whole size as a week's growth.
    pub growth_bytes_7d: Option<i64>,
    pub prefix_breakdown: Option<serde_json::Value>,
    pub measured_at: String,
    pub measure_status: String,
    pub measure_detail: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetStorageResponse {
    pub rows: Vec<AppUsageRow>,
    pub total_bytes: i64,
    pub total_objects: i64,
    pub total_untagged_bytes: i64,
    /// Apps with no usage row yet. Surfaced so "0 GB" is never confused with
    /// "never measured" — the two look identical in a table and mean opposite
    /// things.
    pub unmeasured_apps: u64,
    /// True when any row's last walk was incomplete, so the totals are a floor.
    pub totals_are_floor: bool,
    pub soft_limit_bytes: Option<i64>,
    pub hard_limit_bytes: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetQuery {
    /// `bytes` (default) | `growth` | `untagged`.
    pub sort: Option<String>,
    pub limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowseQuery {
    pub prefix: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowseObject {
    pub key: String,
    /// Key with the silo prefix stripped — what an operator actually reads.
    pub path: String,
    pub size: i64,
    pub content_type: Option<String>,
    pub last_modified: Option<String>,
    /// TTL class today's policy assigns this key (`"30d"`), or `None` for
    /// "kept forever".
    pub expire_after: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowseResponse {
    pub objects: Vec<BrowseObject>,
    pub cursor: Option<String>,
    pub has_more: bool,
    /// The app's declared retention rules, so the UI can explain *why* a key
    /// expires (or doesn't) without re-reading the manifest itself.
    pub retention_rules: Vec<RetentionRuleDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetentionRuleDto {
    pub prefix: String,
    pub expire_after: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteRequest {
    /// Full keys as returned by the browser. Validated against the app's silo
    /// server-side — a forged key from another app is rejected, not deleted.
    pub keys: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteResponse {
    pub deleted: usize,
}

/// Cap on one delete call. A bounded batch keeps an accidental "select all" on a
/// six-figure listing from becoming one unkillable request.
const MAX_DELETE_KEYS: usize = 1000;

fn err(status: StatusCode, code: &str, message: &str) -> axum::response::Response {
    (
        status,
        Json(serde_json::json!({ "error": code, "message": message })),
    )
        .into_response()
}

// ── Fleet view ───────────────────────────────────────────────────────────────

/// The ranked rollup rows a caller may see, `None` scope meaning unbounded.
///
/// Split out of [`fleet`] for the same reason `list_apps_scoped` is split out of
/// `list_apps`: the scope filter is the security-relevant half, and inline in a
/// handler it can only be exercised through HTTP with a forged principal. As a
/// function it is one DB-backed test — which matters because deleting the filter
/// is invisible to every other test in the suite while a bounded App Operator
/// gets back every org's app names and byte totals.
pub async fn fleet_rows_scoped(
    db: &sea_orm::DatabaseConnection,
    orgs: Option<&[Uuid]>,
) -> Result<Vec<app_storage_usage::Model>, sea_orm::DbErr> {
    let mut q = AppStorageUsage::find();
    if let Some(orgs) = orgs {
        // The rollup carries `org_id` denormalized precisely so this needs no
        // join — see the migration's note on why the table is app-shaped.
        q = q.filter(app_storage_usage::Column::OrgId.is_in(orgs.to_vec()));
    }
    q.order_by_desc(app_storage_usage::Column::Bytes)
        .all(db)
        .await
}

/// `GET /api/customer-apps/storage` — every measured app, ranked.
///
/// The full row set is loaded even when the caller passes `limit`, and the limit
/// is applied **after** the totals are computed. Pushing it into the query would
/// be cheaper but wrong: `totalBytes` / `totalUntaggedBytes` / `totalsAreFloor`
/// are fleet-wide figures the header renders, so limiting the scan would silently
/// turn them into "totals of the top N" — a number that looks authoritative and
/// under-reports. Computing them with SQL `SUM` instead is its own trap
/// (`SUM(bigint)` returns `numeric`; see `quota::orgs_over_soft_limit`).
///
/// The scan is one row per **app**, not per object, so it stays in the hundreds.
/// If that stops being true, split it into an aggregate query plus a limited page
/// rather than limiting this one.
///
/// **"Fleet-wide" means the caller's fleet.** `Action::PlatformApps` gates this
/// route, and an App Operator may hold that capability bounded to a few orgs — so
/// the rows, and therefore the totals, are narrowed to the orgs their grant
/// reaches. For a bounded grant "totals of what you can see" is the only figure
/// that means anything; showing the true fleet totals beside a filtered table
/// would both leak the size of the rest of the fleet and label a subtotal as a
/// total. `app_scope_guard` cannot do this for us: it keys on a `{id}` path
/// segment and this route has none — see `handlers::org_in_scope`.
pub async fn fleet(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Query(q): Query<FleetQuery>,
) -> axum::response::Response {
    let Ok(db) = establish_connection().await else {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DbUnavailable",
            "could not reach the database",
        );
    };

    // Lenient (`Err` → unfiltered), matching `list_apps`: this is a list read, so
    // an unreadable grant over-showing beats a staff operator seeing an empty
    // registry during a DB blip. The targeted routes below fail closed instead.
    let scope = super::handlers::scope_org_filter(&db, &user).await;

    let usage = match fleet_rows_scoped(&db, scope.as_deref()).await {
        Ok(u) => u,
        Err(e) => {
            tracing::warn!("storage fleet query failed: {e}");
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "UsageQueryFailed",
                "could not read storage usage",
            );
        }
    };

    // Resolve app + org names in two batched queries rather than per row.
    let app_ids: Vec<Uuid> = usage.iter().map(|u| u.app_id).collect();
    let app_rows = Apps::find()
        .filter(apps::Column::Id.is_in(app_ids.clone()))
        .all(&db)
        .await
        .unwrap_or_default();
    let apps_by_id: std::collections::HashMap<Uuid, apps::Model> =
        app_rows.into_iter().map(|a| (a.id, a)).collect();
    let org_ids: Vec<Uuid> = usage.iter().map(|u| u.org_id).collect();
    let orgs_by_id: std::collections::HashMap<Uuid, String> = Organizations::find()
        .filter(organizations::Column::Id.is_in(org_ids))
        .all(&db)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|o| (o.id, o.name))
        .collect();

    let current: Vec<(Uuid, i64)> = usage.iter().map(|u| (u.app_id, u.bytes)).collect();
    let growth = growth_by_app(&db, &current).await;

    let total_bytes = usage.iter().map(|u| u.bytes).sum();
    let total_objects = usage.iter().map(|u| u.object_count).sum();
    let total_untagged_bytes = usage.iter().map(|u| u.untagged_bytes).sum();
    let totals_are_floor = usage
        .iter()
        .any(|u| u.measure_status != entity::app_storage_usage::measure_status::OK);

    let mut rows: Vec<AppUsageRow> = usage
        .into_iter()
        .map(|u| {
            let app = apps_by_id.get(&u.app_id);
            AppUsageRow {
                app_id: u.app_id,
                app_name: app.map(|a| a.name.clone()).unwrap_or_default(),
                app_slug: app.map(|a| a.slug.clone()).unwrap_or_default(),
                org_id: u.org_id,
                org_name: orgs_by_id.get(&u.org_id).cloned(),
                bytes: u.bytes,
                object_count: u.object_count,
                untagged_bytes: u.untagged_bytes,
                growth_bytes_7d: growth.get(&u.app_id).copied(),
                prefix_breakdown: u.prefix_breakdown,
                measured_at: u.measured_at.to_rfc3339(),
                measure_status: u.measure_status,
                measure_detail: u.measure_detail,
            }
        })
        .collect();

    match q.sort.as_deref() {
        // Growth is the number that predicts the next invoice, so it gets to be
        // a first-class sort rather than something the operator eyeballs.
        Some("growth") => rows.sort_by_key(|r| std::cmp::Reverse(r.growth_bytes_7d.unwrap_or(0))),
        Some("untagged") => rows.sort_by_key(|r| std::cmp::Reverse(r.untagged_bytes)),
        _ => rows.sort_by_key(|r| std::cmp::Reverse(r.bytes)),
    }
    if let Some(limit) = q.limit {
        rows.truncate(limit as usize);
    }

    let unmeasured_apps = storage::sweeper::unmeasured_app_count(&db, scope.as_deref())
        .await
        .unwrap_or_default();

    Json(FleetStorageResponse {
        rows,
        total_bytes,
        total_objects,
        total_untagged_bytes,
        unmeasured_apps,
        totals_are_floor,
        soft_limit_bytes: storage::quota::soft_limit_bytes(),
        hard_limit_bytes: storage::quota::hard_limit_bytes(),
    })
    .into_response()
}

/// The growth window the fleet view reports. Named once so the cutoff and the
/// column header cannot drift apart.
const GROWTH_WINDOW_DAYS: i64 = 7;

/// How far back of a window to search for the baseline sample.
///
/// The baseline is "the newest sample at least 7 days old", but querying that
/// without a lower bound would scan the app's whole retained history — 400 days
/// of samples per app, which at fleet scale is millions of rows to compute one
/// column. Bounding the search to samples aged 7–14 days keeps the scan flat in
/// history length while still catching apps the sweeper only reaches daily.
const GROWTH_BASELINE_WINDOW_DAYS: i64 = 7;

/// Δ bytes over the trailing [`GROWTH_WINDOW_DAYS`] per app.
///
/// `current` comes from the rollup rather than a second sample query — the
/// rollup already holds each app's latest measurement, so the only thing the
/// series is needed for is the baseline.
///
/// An app with no sample in the baseline window yields **no entry** rather than a
/// fabricated delta: reporting a three-day-old app's entire size as "weekly
/// growth" would put every new app at the top of the growth ranking and make the
/// ranking useless for the thing it exists to find.
async fn growth_by_app(
    db: &sea_orm::DatabaseConnection,
    current: &[(Uuid, i64)],
) -> std::collections::HashMap<Uuid, i64> {
    use std::collections::HashMap;
    if current.is_empty() {
        return HashMap::new();
    }
    let now = chrono::Utc::now();
    let cutoff: chrono::DateTime<chrono::FixedOffset> =
        (now - chrono::Duration::days(GROWTH_WINDOW_DAYS)).into();
    let window_start: chrono::DateTime<chrono::FixedOffset> =
        (now - chrono::Duration::days(GROWTH_WINDOW_DAYS + GROWTH_BASELINE_WINDOW_DAYS)).into();

    let app_ids: Vec<Uuid> = current.iter().map(|(id, _)| *id).collect();
    let samples = AppStorageUsageSamples::find()
        .filter(app_storage_usage_samples::Column::AppId.is_in(app_ids))
        .filter(app_storage_usage_samples::Column::MeasuredAt.gte(window_start))
        .filter(app_storage_usage_samples::Column::MeasuredAt.lte(cutoff))
        .order_by_asc(app_storage_usage_samples::Column::MeasuredAt)
        .all(db)
        .await
        .unwrap_or_default();

    // Ascending order, so the last write per app wins: the newest sample that is
    // still at least a week old.
    let mut baseline: HashMap<Uuid, i64> = HashMap::new();
    for s in samples {
        baseline.insert(s.app_id, s.bytes);
    }
    current
        .iter()
        .filter_map(|(app_id, now_bytes)| {
            baseline.get(app_id).map(|base| (*app_id, now_bytes - base))
        })
        .collect()
}

// ── Per-app browser ──────────────────────────────────────────────────────────

async fn app_retention(
    db: &sea_orm::DatabaseConnection,
    app: &apps::Model,
) -> (storage::RetentionPolicy, Vec<RetentionRuleDto>) {
    use entity::prelude::AppBuilds;
    let Some(build_pk) = app.published_build_id.or(app.draft_build_id) else {
        return (storage::RetentionPolicy::default(), Vec::new());
    };
    let manifest = AppBuilds::find_by_id(build_pk)
        .one(db)
        .await
        .ok()
        .flatten()
        .and_then(|b| b.manifest_json);
    let policy = crate::server::api::custom_apps_manifest::retention_policy_from_build_manifest(
        manifest.as_ref(),
        app.id,
    );
    // Echo the declared rules straight from the manifest so the UI can show what
    // the author wrote, including entries the policy dropped as invalid.
    let rules = manifest
        .as_ref()
        .and_then(|m| m.get("storage"))
        .and_then(|s| s.get("retention"))
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    Some(RetentionRuleDto {
                        prefix: v.get("prefix")?.as_str()?.to_string(),
                        expire_after: v
                            .get("expireAfter")
                            .and_then(|e| e.as_str())
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    (policy, rules)
}

/// `GET /api/customer-apps/{id}/storage/objects` — one page of an app's silo.
pub async fn browse(
    Path(app_id): Path<Uuid>,
    Query(q): Query<BrowseQuery>,
) -> axum::response::Response {
    let Ok(db) = establish_connection().await else {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DbUnavailable",
            "could not reach the database",
        );
    };
    let Ok(Some(app)) = Apps::find_by_id(app_id).one(&db).await else {
        return err(StatusCode::NOT_FOUND, "AppNotFound", "no such app");
    };

    let (policy, retention_rules) = app_retention(&db, &app).await;
    let page = match storage::list(
        &storage::Silo::production(app_id),
        q.prefix.as_deref(),
        q.limit,
        q.cursor,
    )
    .await
    {
        Ok(p) => p,
        Err(e) => {
            return err(
                StatusCode::BAD_GATEWAY,
                "StorageListFailed",
                &format!("could not list this app's assets: {e}"),
            );
        }
    };

    let silo = storage::app_prefix(app_id);
    let objects = page
        .objects
        .into_iter()
        .map(|o| {
            let path = o
                .key
                .strip_prefix(silo.as_str())
                .unwrap_or(&o.key)
                .to_string();
            BrowseObject {
                expire_after: policy.resolve(&path).map(|c| c.tag_value().to_string()),
                path,
                key: o.key,
                size: o.size,
                content_type: o.content_type,
                last_modified: o.last_modified,
            }
        })
        .collect();

    Json(BrowseResponse {
        objects,
        cursor: page.cursor,
        has_more: page.has_more,
        retention_rules,
    })
    .into_response()
}

/// `POST /api/customer-apps/{id}/storage/delete` — remove selected objects.
pub async fn delete_objects(
    _: crate::server::api::custom_apps_agent_refusal::RefuseSandboxAgent,
    Path(app_id): Path<Uuid>,
    Json(body): Json<DeleteRequest>,
) -> axum::response::Response {
    if body.keys.is_empty() {
        return err(
            StatusCode::BAD_REQUEST,
            "NoKeys",
            "no keys were supplied to delete",
        );
    }
    if body.keys.len() > MAX_DELETE_KEYS {
        return err(
            StatusCode::BAD_REQUEST,
            "TooManyKeys",
            &format!("delete at most {MAX_DELETE_KEYS} keys per request"),
        );
    }
    // `storage::delete` re-validates every key against this app's silo, so a key
    // belonging to another app is refused here rather than trusted because it
    // arrived from an admin session.
    match storage::delete(&storage::Silo::production(app_id), &body.keys).await {
        Ok(deleted) => {
            tracing::info!(%app_id, deleted, "admin deleted custom-app assets");
            Json(DeleteResponse { deleted }).into_response()
        }
        Err(storage::StorageError::Denied(m)) => err(StatusCode::FORBIDDEN, "KeyDenied", &m),
        Err(e) => err(
            StatusCode::BAD_GATEWAY,
            "StorageDeleteFailed",
            &format!("could not delete: {e}"),
        ),
    }
}

/// The scope of the manual sweep running **in this process**, if one is.
///
/// `None` = idle. `Some(None)` = an unbounded sweep. `Some(Some(orgs))` = a sweep
/// bounded to those orgs.
///
/// A sweep is up to `DEFAULT_BATCH_SIZE` apps × `MAX_MEASURE_PAGES` sequential
/// S3 LISTs, so two impatient clicks would double the LIST bill; the second is
/// refused rather than queued.
///
/// **It stores the scope, not just a flag, because scoped sweeps are not
/// interchangeable.** While every sweep was fleet-wide, refusing the second was
/// free — it would have measured exactly what the first is already measuring. Now
/// a sweep bounded to org X and one bounded to org Y walk disjoint app sets, so a
/// bare flag serializes work that isn't equivalent and answers "a sweep is already
/// running" to an operator whose orgs that sweep will never touch. They then poll
/// a table that will not change. [`covers`] is what lets the 409 say which case
/// it is.
///
/// It is deliberately **not** a distributed lock. Two operators hitting two serve
/// replicas will each start a walk, and nothing stops them — unlike the periodic
/// sweeper, which is singleton-gated at the composition root. The cost of that is
/// duplicated LISTs, not corruption: `persist` is an idempotent upsert and the
/// sample key is `(app, measured_at)`, so overlapping sweeps converge on the same
/// rows. If the LIST bill ever justifies it, promote this to an advisory lock in
/// Postgres rather than trusting process-local state.
static SWEEP_IN_FLIGHT: std::sync::Mutex<Option<Option<Vec<Uuid>>>> = std::sync::Mutex::new(None);

/// Does a sweep over `running` also measure everything `requested` wants?
///
/// Answers the only question the 409 needs: whether waiting for the running sweep
/// will actually refresh the caller's rows. An unbounded sweep covers every
/// request; a bounded one covers only requests whose orgs it already includes,
/// and never covers an unbounded request.
fn covers(running: &Option<Vec<Uuid>>, requested: &Option<Vec<Uuid>>) -> bool {
    match (running, requested) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(running), Some(requested)) => requested.iter().all(|o| running.contains(o)),
    }
}

/// What [`try_claim_sweep`] decided, and therefore which response the caller owes.
#[derive(Debug, PartialEq, Eq)]
enum SweepClaim {
    /// Slot taken; the caller must spawn the walk and release it when done.
    Claimed,
    /// The grant reaches no orgs, so there is nothing to measure. No slot taken.
    NothingToDo,
    /// A sweep is already running. `covered` says whether waiting for it will
    /// actually refresh this caller's rows — the difference between the two 409s.
    Busy { covered: bool },
}

/// Decide whether this request may sweep, taking the slot if so.
///
/// Split from the handler so all three outcomes are testable: the branch that
/// matters most — an empty grant scope must NOT take the slot — is otherwise
/// reachable only through HTTP with a forged principal, and its cost is
/// invisible (a request that does no work briefly blocking ones that would).
fn try_claim_sweep(scope: &Option<Vec<Uuid>>) -> SweepClaim {
    if scope.as_deref().is_some_and(<[Uuid]>::is_empty) {
        return SweepClaim::NothingToDo;
    }
    let mut in_flight = SWEEP_IN_FLIGHT.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(running) = in_flight.as_ref() {
        return SweepClaim::Busy {
            covered: covers(running, scope),
        };
    }
    *in_flight = Some(scope.clone());
    SweepClaim::Claimed
}

/// `POST /api/customer-apps/storage/sweep` — kick off a re-measure.
///
/// An operator looking at a number they distrust should be able to refresh it
/// rather than wait out the interval; without this the fleet view's staleness is
/// visible but unactionable.
///
/// **Returns 202 immediately** and sweeps in the background. Running it inline
/// held the request (and a DB connection) for as long as the walk took, with no
/// timeout — minutes for a large fleet. The client polls the fleet endpoint,
/// whose `measuredAt` column already shows when each row was last refreshed, so
/// there is nothing the synchronous response told the operator that the table
/// does not.
///
/// The walk is bounded to the caller's grant. Nothing in the request names a
/// target, so `app_scope_guard` has nothing to key on and an operator scoped to
/// one org would otherwise start a walk over every app in the fleet — a cost
/// amplifier rather than a disclosure, but the same missing check.
pub async fn sweep_now(
    _: crate::server::api::custom_apps_agent_refusal::RefuseSandboxAgent,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
) -> axum::response::Response {
    let Ok(db) = establish_connection().await else {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DbUnavailable",
            "could not reach the database",
        );
    };

    // Fails closed: this one spends money, so an unreadable grant must not
    // degrade to "sweep everything".
    let scope = match super::handlers::scope_org_filter_checked(&db, &user).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(target: "authz", error = %e, "platform grant unreadable on a storage SWEEP — refusing");
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "ScopeUnavailable",
                "could not resolve your access scope",
            );
        }
    };

    match try_claim_sweep(&scope) {
        SweepClaim::Claimed => {}
        // A grant bounded to NO orgs: nothing to measure, and taking the slot to
        // discover that would block operators who do have work. Honest 202 —
        // the request was accepted and completed, it just found no apps.
        SweepClaim::NothingToDo => {
            return (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({ "started": false, "reason": "no apps in scope" })),
            )
                .into_response();
        }
        SweepClaim::Busy { covered: true } => {
            let batch = storage::sweeper::batch_size();
            return err(
                StatusCode::CONFLICT,
                "SweepInFlight",
                // NOT "covering your apps", and NOT "the {batch} stalest apps".
                // A sweep measures at most `batch_size` apps, oldest first,
                // drawn from ITS OWN scope — which may be wider than the
                // caller's, so an unbounded run's 24 stalest can be entirely
                // outside their orgs and this 409 can precede zero of their rows
                // changing. "up to … in its own scope" is the true version; the
                // count is stated because `measuredAt` in the table they are
                // about to stare at is what makes it checkable.
                &format!(
                    "a storage sweep covering your orgs is already running; it refreshes \
                     up to {batch} of the stalest apps in its own scope, so wait for it \
                     to finish and run it again if rows are still stale"
                ),
            );
        }
        // Waiting would not help for at least part of this request: the running
        // sweep's scope does not contain all of the caller's. A flat "already
        // running" is what sends someone off to poll a table that cannot change.
        SweepClaim::Busy { covered: false } => {
            return err(
                StatusCode::CONFLICT,
                "SweepInFlightElsewhere",
                // "all of" is load-bearing. An UNBOUNDED caller lands here
                // whenever any bounded sweep runs, and that sweep does cover
                // some of their apps — just not every one.
                "another storage sweep is running that does not cover all of your apps; \
                 retry once it finishes",
            );
        }
    }

    // Built HERE, not inside the async block. A future dropped before its first
    // poll — runtime shutdown between spawn and schedule — would never run the
    // block, so a guard constructed inside it would never exist and the claim
    // would leak for the process lifetime, 409'ing every later sweep. Moving an
    // already-live guard in means dropping the future drops the guard.
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            // `into_inner` on a poisoned lock: a panic mid-sweep already left
            // the claim set, and refusing to clear it here is exactly the stuck
            // claim this guard exists to prevent.
            *SWEEP_IN_FLIGHT.lock().unwrap_or_else(|p| p.into_inner()) = None;
        }
    }
    let guard = Guard;

    tokio::spawn(async move {
        // Released however this ends, including on a panic inside the sweep.
        let _guard = guard;

        match storage::sweeper::sweep_once(&db, scope.as_deref()).await {
            Ok(report) => tracing::info!(
                measured = report.measured,
                failed = report.failed,
                // A sweep where every walk was truncated still reports
                // "measured N, failed 0" without this.
                incomplete = report.incomplete,
                "manual storage sweep finished"
            ),
            Err(e) => tracing::warn!(error = %e, "manual storage sweep failed"),
        }
    });

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "started": true })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryQuery {
    /// Window length. Clamped — a caller asking for 10 years would scan the
    /// whole retained series to draw a chart nobody can read.
    pub days: Option<i64>,
    /// Omit for the fleet total; set to chart one app.
    pub app_id: Option<Uuid>,
}

/// Longest window the chart offers. Samples are pruned at 400 days, so anything
/// past a year is mostly empty anyway.
const MAX_HISTORY_DAYS: i64 = 365;

/// `GET /api/customer-apps/storage/history` — daily totals for the usage chart.
///
/// Each point is the value **held at that day's end**, carried forward per app.
/// Storage is a level, not a flow: summing a day's samples would make a day with
/// two measurements look twice as large and a day with none look empty, both of
/// which are artifacts of the sweep schedule rather than anything a tenant did.
///
/// The app id arrives as a **query param**, which `app_scope_guard` cannot see —
/// it keys on `{id}` in the path. So this handler checks scope itself: an app in
/// an org the caller's grant doesn't reach gets the same 404 an unknown id gets,
/// and the fleet-total form (`appId` omitted) is narrowed to the orgs in reach.
///
/// **Which form decides how an unreadable grant is treated**, not the route. With
/// `appId` this is a targeted read and fails closed; without it, it is the chart
/// above [`fleet`]'s table — the same list read, on the same screen, from the same
/// rows — so it takes the same lenient path. Splitting that the other way put a
/// 500'd chart above an over-showing table during a single grants-table blip, and
/// neither half's rule predicted which you'd get.
pub async fn history(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Query(q): Query<HistoryQuery>,
) -> axum::response::Response {
    let Ok(db) = establish_connection().await else {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DbUnavailable",
            "could not reach the database",
        );
    };

    let scope = if let Some(app_id) = q.app_id {
        let scope = match super::handlers::scope_org_filter_checked(&db, &user).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(target: "authz", error = %e, "platform grant unreadable on a targeted storage history — refusing");
                return err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "ScopeUnavailable",
                    "could not resolve your access scope",
                );
            }
        };

        // Resolve the app's owning org and check it directly. Passing the org
        // scope to `load_history` and letting it return an empty series would
        // read as "this app has no history" — indistinguishable from a real
        // brand-new app, and it would answer at all for an app the caller may
        // not know exists.
        let owning_org = match Apps::find_by_id(app_id).one(&db).await {
            Ok(a) => a.map(|a| a.org_id),
            Err(e) => {
                tracing::error!("history scope lookup failed: {e}");
                return err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "ScopeUnavailable",
                    "could not resolve the app",
                );
            }
        };
        let reachable = match (&scope, owning_org) {
            (None, _) => true,                              // unbounded grant
            (Some(orgs), Some(org)) => orgs.contains(&org), // bounded: must be in reach
            // No app row. Fall through to the empty series rather than 404'ing
            // here, so a bounded operator can't tell an unknown id from one that
            // exists in an org they can't reach — same rule as `split_by_scope`.
            (Some(_), None) => true,
        };
        if !reachable {
            return err(StatusCode::NOT_FOUND, "AppNotFound", "app not found");
        }
        scope
    } else {
        // Fleet form: the same list read `fleet` performs, so the same lenient
        // filter — see this function's doc comment.
        super::handlers::scope_org_filter(&db, &user).await
    };

    let days = q.days.unwrap_or(30).clamp(1, MAX_HISTORY_DAYS);
    match storage::metering::load_history(&db, q.app_id, scope.as_deref(), days).await {
        Ok(points) => Json(serde_json::json!({ "days": days, "points": points })).into_response(),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "HistoryFailed",
            &format!("could not load usage history: {e}"),
        ),
    }
}

/// `GET /api/customer-apps/storage/meter/{org_id}` — month-to-date GB-month.
///
/// The target org is `{org_id}`, not `{id}`, so `app_scope_guard` passes it
/// through untouched — this is the billing figure for one named tenant, so it
/// checks scope itself and 404s (not 403s) when the grant doesn't reach it.
pub async fn org_meter(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(org_id): Path<Uuid>,
) -> axum::response::Response {
    let Ok(db) = establish_connection().await else {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DbUnavailable",
            "could not reach the database",
        );
    };
    match super::handlers::org_in_scope(&db, &user, org_id).await {
        Ok(true) => {}
        Ok(false) => return err(StatusCode::NOT_FOUND, "OrgNotFound", "org not found"),
        Err(status) => {
            return err(
                status,
                "ScopeUnavailable",
                "could not resolve your access scope",
            );
        }
    }
    match storage::metering::meter_org_month_to_date(&db, org_id).await {
        Ok(m) => Json(serde_json::json!({
            "orgId": m.org_id,
            "periodStart": m.period_start.to_rfc3339(),
            "periodEnd": m.period_end.to_rfc3339(),
            "gibMonth": m.gib_month,
            "averageBytes": m.average_bytes,
            "apps": m.apps.iter().map(|a| serde_json::json!({
                "appId": a.app_id,
                "gibMonth": a.gib_month,
                "sampleCount": a.sample_count,
            })).collect::<Vec<_>>(),
            // Non-empty means the figure UNDER-counts; never invoice past this
            // without looking.
            "appsWithoutSamples": m.apps_without_samples,
        }))
        .into_response(),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "MeterFailed",
            &format!("could not meter org: {e}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `covers` decides which of two opposite 409s an operator gets, and the
    /// containment is the easy thing to write backwards — swapping the operands
    /// to `running.iter().all(|o| requested.contains(o))` type-checks, reads
    /// plausibly, and tells every bounded operator to wait for a sweep that will
    /// never touch their orgs. That is the bug this function was added to fix,
    /// reintroduced with the suite still green.
    #[test]
    fn covers_answers_whether_waiting_would_actually_help() {
        let (a, b) = (Uuid::from_u128(1), Uuid::from_u128(2));

        // An unbounded sweep measures everything, so waiting always helps.
        assert!(covers(&None, &None));
        assert!(covers(&None, &Some(vec![a])));

        // A bounded sweep never satisfies an unbounded request: it leaves every
        // org outside its scope unmeasured.
        assert!(!covers(&Some(vec![a]), &None));

        // Superset covers subset; disjoint does not.
        assert!(covers(&Some(vec![a, b]), &Some(vec![a])));
        assert!(!covers(&Some(vec![a]), &Some(vec![b])));
        assert!(!covers(&Some(vec![a]), &Some(vec![a, b])));
    }

    /// `SWEEP_IN_FLIGHT` is a process-wide static, so these rely on nextest's
    /// process-per-test isolation (which this repo mandates) for a fresh slot.
    #[test]
    fn an_empty_grant_scope_does_not_take_the_sweep_slot() {
        // A grant reaching no orgs has nothing to measure: `apps_due` with an
        // empty filter returns zero apps. Letting it claim anyway means a
        // request that does no work blocks ones that would — every other
        // operator gets `SweepInFlightElsewhere`, since `covers(&[], _)` is
        // false for every non-empty and unbounded request. Brief, but backwards.
        assert_eq!(try_claim_sweep(&Some(vec![])), SweepClaim::NothingToDo);
        // The slot must still be free afterwards — that is the whole point.
        assert_eq!(try_claim_sweep(&None), SweepClaim::Claimed);
    }

    #[test]
    fn a_second_sweep_is_told_whether_waiting_would_help() {
        let (a, b) = (Uuid::from_u128(1), Uuid::from_u128(2));
        assert_eq!(try_claim_sweep(&Some(vec![a])), SweepClaim::Claimed);

        // Same orgs, or a subset: waiting refreshes their rows.
        assert_eq!(
            try_claim_sweep(&Some(vec![a])),
            SweepClaim::Busy { covered: true }
        );
        // Disjoint, and unbounded: the running sweep will never finish their
        // work, so "wait for it" would be a lie.
        assert_eq!(
            try_claim_sweep(&Some(vec![b])),
            SweepClaim::Busy { covered: false }
        );
        assert_eq!(try_claim_sweep(&None), SweepClaim::Busy { covered: false });
    }

    #[test]
    fn a_request_scoped_to_no_orgs_is_vacuously_covered() {
        // `Scope::Orgs(vec![])` asks for nothing, so any running sweep trivially
        // "covers" it and the caller is told to wait. Stated as a decision
        // rather than left as fallout from `all()` on an empty iterator: such a
        // grant has no rows to refresh either way, so both 409s are equally
        // true and the softer one is less alarming. If zero-scope grants ever
        // become a state worth surfacing, this is the line to revisit.
        assert!(covers(&Some(vec![Uuid::from_u128(1)]), &Some(vec![])));
        assert!(covers(&None, &Some(vec![])));
    }

    #[test]
    fn delete_cap_is_enforced_as_a_constant_not_a_magic_number() {
        // Guards the contract the handler advertises in its error text.
        assert_eq!(MAX_DELETE_KEYS, 1000);
    }

    #[test]
    fn browse_object_path_strips_the_silo_prefix() {
        let app_id = Uuid::from_u128(7);
        let silo = storage::app_prefix(app_id);
        let key = format!("{silo}generated/q1.pdf");
        let path = key.strip_prefix(silo.as_str()).unwrap();
        assert_eq!(path, "generated/q1.pdf");
    }
}
