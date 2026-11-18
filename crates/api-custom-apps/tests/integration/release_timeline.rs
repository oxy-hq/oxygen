//! The release timeline (`GET /customer-apps/{id}/releases`).
//!
//! | Module | Proves |
//! | --- | --- |
//! | `seeded` | `releases_of` over hand-seeded history with explicit times: one release per production stretch, an anchor clipped to the window, a pruned build with no metrics that `previous` skips, a rollback a release of its own; `shed` and `running` are not calls, a `success` that answered 503 is a failure, and a call counts only inside its own build's stretch; red on the failure rule with p95 and LCP reasons beside it; a migration on the release whose promote ran it and one no release claims; a failure first seen on a build is that release's and an older one is not new; vitals read only from rows naming their build, bounded in SQL (`9e9`, `1e400` and a string are not samples); a red release serving now names its previous build, one superseded does not; a slow part answers `unavailable` alone |
//! | `route` | through the real routers in a staff session: three lanes, staging's publishes, a sandbox's event, a red current release whose `rollback.to` the dry-run plan names; `days` out of range is `400 invalid_days`; a sandbox agent token is 404; a publish token and a caller without non-production reach get production only, `not_permitted` |

use chrono::{DateTime, Duration, TimeZone, Utc};
use entity::{app_builds, app_environment_events, app_function_invocations, apps};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ConnectionTrait, DatabaseConnection, DbBackend,
    EntityTrait, Statement,
};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::Tenant;

mod route;
mod seeded;

/// The fixed "now" the seeded cases read at.
pub(crate) fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 14, 5, 0, 0).unwrap()
}

pub(crate) fn days_ago(d: i64) -> DateTime<Utc> {
    now() - Duration::days(d)
}

/// A bare app row in `t`'s org, with no build and no history.
pub(crate) async fn bare_app(t: &Tenant) -> apps::Model {
    let id = Uuid::new_v4();
    apps::ActiveModel {
        id: Set(id),
        org_id: Set(t.org_id),
        project_id: Set(demo_workspace_id()),
        slug: Set(format!("timeline-{}", id.simple())),
        name: Set("Timeline".into()),
        branch: Set("main".into()),
        source_repo: Set("timeline/app".into()),
        status: Set("active".into()),
        source_type: Set("s3".into()),
        source_config: Set(json!({})),
        visibility: Set("org".into()),
        ..Default::default()
    }
    .insert(&t.db)
    .await
    .expect("seed the app")
}

/// A build of `app` published as `publish_id`.
pub(crate) async fn build(db: &DatabaseConnection, app: Uuid, publish_id: &str) -> Uuid {
    let id = Uuid::new_v4();
    app_builds::ActiveModel {
        id: Set(id),
        app_id: Set(app),
        build_id: Set(publish_id.into()),
        s3_prefix: Set(format!("apps/{publish_id}")),
        created_at: Set(days_ago(60).fixed_offset()),
        validation_status: Set("passed".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed the build");
    id
}

/// A pointer move of `environment` to `build` at `at`.
pub(crate) async fn moved(
    db: &DatabaseConnection,
    app: Uuid,
    environment: &str,
    build: Option<Uuid>,
    action: &str,
    at: DateTime<Utc>,
) {
    app_environment_events::ActiveModel {
        id: Set(Uuid::new_v4()),
        app_id: Set(app),
        environment: Set(environment.into()),
        build_id: Set(build),
        action: Set(action.into()),
        actor: Set(None),
        at: Set(at.fixed_offset()),
    }
    .insert(db)
    .await
    .expect("seed the event");
}

/// What a run of seeded calls looks like.
#[derive(Clone, Copy)]
pub(crate) struct Calls {
    pub status: &'static str,
    pub mode: &'static str,
    pub duration_ms: i64,
    pub result_status: Option<i16>,
    pub fingerprint: Option<&'static str>,
}

pub(crate) const OK: Calls = Calls {
    status: "success",
    mode: "route",
    duration_ms: 100,
    result_status: None,
    fingerprint: None,
};

/// `n` production calls of `build`, one a minute from `from`.
pub(crate) async fn calls(
    db: &DatabaseConnection,
    app: Uuid,
    build: Uuid,
    from: DateTime<Utc>,
    n: usize,
    c: Calls,
) {
    let models = (0..n).map(|k| app_function_invocations::ActiveModel {
        id: Set(Uuid::new_v4()),
        app_id: Set(app),
        build_id: Set(build),
        function_name: Set("store".into()),
        mode: Set(c.mode.into()),
        user_id: Set(None),
        status: Set(c.status.into()),
        duration_ms: Set(Some(c.duration_ms)),
        error: Set(None),
        cancel_requested_at: Set(None),
        created_at: Set((from + Duration::minutes(k as i64)).fixed_offset()),
        idempotency_key: Set(None),
        result_body: Set(None),
        request_hash: Set(None),
        result_status: Set(c.result_status),
        failure_fingerprint: Set(c.fingerprint.map(str::to_string)),
        environment: Set("production".into()),
        credential_token_id: Set(None),
        parent_invocation_id: Set(None),
        error_kind: Set(None),
    });
    app_function_invocations::Entity::insert_many(models)
        .exec_without_returning(db)
        .await
        .expect("seed the calls");
}

/// One `oxy-web-vitals` row, its payload given as JSON text so a value serde
/// would refuse (`1e400`) can be stored as Postgres would take it.
pub(crate) async fn vitals_row(
    db: &DatabaseConnection,
    app: Uuid,
    user: Uuid,
    at: DateTime<Utc>,
    payload: &str,
) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO custom_app_event (id, app_id, user_id, user_email, session_id, \
         event_name, payload, occurred_at, environment) \
         VALUES ($1, $2, $3, 'viewer@timeline.test', $4, 'oxy-web-vitals', $5::jsonb, $6, \
         'production')",
        [
            Uuid::new_v4().into(),
            app.into(),
            user.into(),
            Uuid::new_v4().into(),
            payload.into(),
            at.into(),
        ],
    ))
    .await
    .expect("seed a vitals row");
}

/// A production migration ledger row.
pub(crate) async fn applied(
    db: &DatabaseConnection,
    app: Uuid,
    filename: &str,
    build: Option<Uuid>,
    at: DateTime<Utc>,
) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO custom_app_migrations (app_id, store, target, filename, checksum, \
         applied_at, applied_by_build) VALUES ($1, 'oltp', 'production', $2, 'x', $3, $4)",
        [app.into(), filename.into(), at.into(), build.into()],
    ))
    .await
    .expect("seed a migration");
}

/// The release with `index` in a timeline answer.
pub(crate) fn release(body: &Value, index: u64) -> &Value {
    body["releases"]
        .as_array()
        .expect("releases")
        .iter()
        .find(|r| r["index"] == index)
        .unwrap_or_else(|| panic!("no release {index}: {body:#}"))
}
