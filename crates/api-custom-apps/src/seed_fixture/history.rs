//! Production's invocation history before the seed: deterministic
//! `app_function_invocations` rows for every stretch of the calendar.
//!
//! Past traffic cannot be produced for real (a call runs now, on the build
//! production serves now), so it is written as rows: the same columns, the
//! same bookkeeping the runner writes, no `result_body`, and a failure's
//! fingerprint from the runner's own rule (`seeded_fingerprint`). The release
//! timeline reads these per stretch. They carry no per-call trace, which is
//! true: nothing ran.
//!
//! The traffic, per hour of every stretch: `stores` 6 route calls, `store` 4
//! (one in four for a store id that no longer exists — a stale link), the
//! `snapshot` schedule on each quarter hour, and one `smoke` check a day.
//! Every release answers the stale id with a 404 except r4, the bad one, whose
//! `store` throws there and is several times slower everywhere. No release
//! but r4 has a failed row, so its failure is the only issue the history holds.

use chrono::{DateTime, Duration, DurationRound, Utc};
use entity::app_function_invocations::{self, ActiveModel, Column, Entity};
use sea_orm::sea_query::OnConflict;
use sea_orm::{ActiveValue::Set, ConnectionTrait, DbErr, EntityTrait};
use uuid::Uuid;

use super::calendar::{Calendar, LIVE, Stretch};
use crate::custom_apps_functions::{seeded_error_kind, seeded_fingerprint};

/// What r4's `store` stores for an unknown id: the TypeError the real runtime
/// records for `customer-apps/examples/pulse-demo/r4/functions/store.js`. The
/// integration test `demo_fixture` runs that file through the real runner and
/// checks that its fingerprint equals the seeded rows', so a change to the
/// file or to the runtime's error text fails there.
pub const R4_STORE_ERROR: &str = "function threw: TypeError: Cannot read properties of undefined \
     (reading 'store')\n    at default (oxy:function:81:39)\n    at async oxy:invoke:11:25";

/// Rows per multi-row insert.
const CHUNK: usize = 500;

/// One seeded invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seeded {
    pub id: Uuid,
    pub release: usize,
    pub function: &'static str,
    pub mode: &'static str,
    /// The viewer for a route call; `None` for a schedule or a check.
    pub user: Option<Uuid>,
    pub status: &'static str,
    pub duration_ms: i64,
    pub error: Option<&'static str>,
    pub fingerprint: Option<String>,
    /// Stored with the fingerprint, as the runner stores them together.
    pub error_kind: Option<&'static str>,
    pub at: DateTime<Utc>,
}

/// Every seeded invocation of every stretch, oldest stretch first.
pub fn invocations(app_id: Uuid, cal: &Calendar, viewer: Uuid) -> Vec<Seeded> {
    cal.stretches()
        .iter()
        .flat_map(|s| stretch_invocations(app_id, s, viewer))
        .collect()
}

fn stretch_invocations(app_id: Uuid, s: &Stretch, viewer: Uuid) -> Vec<Seeded> {
    let mut out = Vec::new();
    let every = |offset_min: i64, step_min: i64| {
        ticks(
            s,
            Duration::minutes(offset_min),
            Duration::minutes(step_min),
        )
    };
    for (n, at) in every(3, 10).into_iter().enumerate() {
        let ms = 140 + scaled(app_id, s, "stores", n, 160);
        out.push(route(app_id, s, "stores", n, at, viewer, ms));
    }
    for (n, at) in every(7, 15).into_iter().enumerate() {
        out.push(store_call(app_id, s, n, at, viewer));
    }
    for (n, at) in quarter_hours(s).into_iter().enumerate() {
        let ms = 300 + scaled(app_id, s, "snapshot", n, 400);
        out.push(system(app_id, s, "snapshot", "schedule", n, at, ms));
    }
    for (n, at) in every(60, 24 * 60).into_iter().enumerate() {
        let ms = 200 + scaled(app_id, s, "smoke", n, 200);
        out.push(system(app_id, s, "smoke", "manual", n, at, ms));
    }
    out
}

/// One `store` route call: one in four asks for a store that no longer exists.
fn store_call(app_id: Uuid, s: &Stretch, n: usize, at: DateTime<Utc>, viewer: Uuid) -> Seeded {
    let stale = n % 4 == 3;
    let bad = s.release == LIVE;
    let ms = match (bad, stale) {
        (false, false) => 180 + scaled(app_id, s, "store", n, 220),
        (false, true) => 120 + scaled(app_id, s, "store", n, 80),
        (true, false) => 1100 + scaled(app_id, s, "store", n, 1300),
        (true, true) => 900 + scaled(app_id, s, "store", n, 500),
    };
    let mut row = route(app_id, s, "store", n, at, viewer, ms);
    if bad && stale {
        row.status = "error";
        row.error = Some(R4_STORE_ERROR);
        row.fingerprint = seeded_fingerprint("error", 0, Some(R4_STORE_ERROR));
        row.error_kind = seeded_error_kind("error", 0, Some(R4_STORE_ERROR));
    }
    row
}

fn route(
    app_id: Uuid,
    s: &Stretch,
    function: &'static str,
    n: usize,
    at: DateTime<Utc>,
    viewer: Uuid,
    duration_ms: i64,
) -> Seeded {
    Seeded {
        user: Some(viewer),
        ..system(app_id, s, function, "route", n, at, duration_ms)
    }
}

fn system(
    app_id: Uuid,
    s: &Stretch,
    function: &'static str,
    mode: &'static str,
    n: usize,
    at: DateTime<Utc>,
    duration_ms: i64,
) -> Seeded {
    Seeded {
        id: seeded_id(app_id, s.release, function, n),
        release: s.release,
        function,
        mode,
        user: None,
        status: "success",
        duration_ms,
        error: None,
        fingerprint: None,
        error_kind: None,
        at,
    }
}

/// `start + offset`, then every `step`, while before `end`.
fn ticks(s: &Stretch, offset: Duration, step: Duration) -> Vec<DateTime<Utc>> {
    std::iter::successors(Some(s.start + offset), |t| Some(*t + step))
        .take_while(|t| *t < s.end)
        .collect()
}

/// Every wall-clock quarter hour in the stretch, as `*/15` fires.
fn quarter_hours(s: &Stretch) -> Vec<DateTime<Utc>> {
    let quarter = Duration::minutes(15);
    let first = s.start.duration_trunc(quarter).unwrap_or(s.start) + quarter;
    ticks(&Stretch { start: first, ..*s }, Duration::zero(), quarter)
}

/// The deterministic id of seeded invocation `n` of `function` in `release`.
pub fn seeded_id(app_id: Uuid, release: usize, function: &str, n: usize) -> Uuid {
    Uuid::new_v5(
        &app_id,
        format!("invocation/r{release}/{function}/{n}").as_bytes(),
    )
}

/// A deterministic value in `0..range` for one call.
fn scaled(app_id: Uuid, s: &Stretch, function: &str, n: usize, range: i64) -> i64 {
    let seed = seeded_id(app_id, s.release, function, n).as_u128() as u64;
    (splitmix64(seed) % range as u64) as i64
}

pub(super) fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Write `rows`, each to its release's build (`builds[k]` is release `k + 1`'s
/// `app_builds.id`). A row that already exists is left alone.
pub async fn write_invocations<C: ConnectionTrait>(
    conn: &C,
    app_id: Uuid,
    builds: &[Uuid],
    rows: &[Seeded],
) -> Result<usize, DbErr> {
    for chunk in rows.chunks(CHUNK) {
        let models = chunk
            .iter()
            .map(|r| model(app_id, builds[r.release - 1], r));
        Entity::insert_many(models)
            .on_conflict(OnConflict::column(Column::Id).do_nothing().to_owned())
            .exec_without_returning(conn)
            .await?;
    }
    Ok(rows.len())
}

fn model(app_id: Uuid, build: Uuid, r: &Seeded) -> ActiveModel {
    app_function_invocations::ActiveModel {
        id: Set(r.id),
        app_id: Set(app_id),
        build_id: Set(build),
        function_name: Set(r.function.to_string()),
        mode: Set(r.mode.to_string()),
        user_id: Set(r.user),
        status: Set(r.status.to_string()),
        duration_ms: Set(Some(r.duration_ms)),
        error: Set(r.error.map(str::to_string)),
        cancel_requested_at: Set(None),
        created_at: Set(r.at.fixed_offset()),
        idempotency_key: Set(None),
        result_body: Set(None),
        request_hash: Set(None),
        result_status: Set(None),
        failure_fingerprint: Set(r.fingerprint.clone()),
        error_kind: Set(r.error_kind.map(str::to_string)),
        environment: Set("production".to_string()),
        credential_token_id: Set(None),
        parent_invocation_id: Set(None),
    }
}
