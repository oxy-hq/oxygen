//! The planes seam of the environment routes
//! (`internal-docs/2026-11-02-custom-app-branches-design.md` §5.5), through the
//! staff console's guards: `PUT …/planes` replaces the stored object and
//! audits it, production has none, a bad choice is refused and leaves the row
//! alone; `POST …/environments` records `from`, `at`, `preset` and `planes`;
//! `POST …/reset` refuses a plane with no reset (`sandbox_reset` has the rest). Who is let
//! through is `sandbox_routes_guards`'.

use axum::http::StatusCode;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::custom_app_functions_fixture::seeded_tenant;
use crate::sandbox_routes::{audit_rows, send, staffed_app};

/// The row's `planes` column, as stored.
async fn stored_planes(db: &DatabaseConnection, app: Uuid, name: &str) -> Option<Value> {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT planes FROM app_environments WHERE app_id = $1 AND name = $2",
        [app.into(), name.into()],
    ))
    .await
    .expect("read the row")
    .map(|row| row.try_get("", "planes").expect("planes"))
}

async fn create(app: Uuid, body: Value) -> (StatusCode, Value) {
    send("POST", &format!("{app}/environments"), Some(body)).await
}

async fn put_planes(app: Uuid, name: &str, planes: Value) -> (StatusCode, Value) {
    send(
        "PUT",
        &format!("{app}/environments/{name}/planes"),
        Some(json!({ "planes": planes })),
    )
    .await
}

async fn reset(app: Uuid, planes: Value) -> (StatusCode, Value) {
    send(
        "POST",
        &format!("{app}/environments/dev-a1/reset"),
        Some(json!({ "planes": planes })),
    )
    .await
}

/// A set of `{a}` and then `{b}` stores exactly `{b}`: the body replaces, it
/// does not merge. Each change is audited, and the environment shows the
/// object as stored.
#[tokio::test]
async fn the_put_replaces_the_stored_object() {
    let t = seeded_tenant().await;
    let app = staffed_app(&t).await;
    let (status, created) = create(app, json!({ "name": "dev-a1" })).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["planes_set"], json!({}), "nothing set yet");

    let a = json!({ "workspace-actions": "refused" });
    let (status, shown) = put_planes(app, "dev-a1", a.clone()).await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    assert_eq!(shown["planes_set"], a);
    assert_eq!(shown["planes"]["workspace-actions"], "refused");
    assert_eq!(stored_planes(&t.db, app, "dev-a1").await, Some(a));

    let b = json!({ "fetch": "held-writes" });
    let (status, shown) = put_planes(app, "dev-a1", b.clone()).await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    assert_eq!(shown["planes_set"], b);
    assert_eq!(
        shown["planes"]["workspace-actions"], "held",
        "a plane left out is its default again"
    );
    assert_eq!(stored_planes(&t.db, app, "dev-a1").await, Some(b.clone()));
    assert_eq!(
        audit_rows(&t.db, app, "app.environment.planes_set").await,
        2
    );

    let (_, again) = send("GET", &format!("{app}/environments/dev-a1"), None).await;
    assert_eq!(again["planes_set"], b);
    let (status, empty) = put_planes(app, "dev-a1", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{empty}");
    assert_eq!(stored_planes(&t.db, app, "dev-a1").await, Some(json!({})));
}

/// Production has no planes: `409 production_has_no_planes`, before the body
/// is looked at, and nothing is written. A reset says the same.
#[tokio::test]
async fn production_answers_409_production_has_no_planes() {
    let t = seeded_tenant().await;
    let app = staffed_app(&t).await;
    for body in [
        json!({ "workspace-actions": "held" }),
        json!({ "bogus": 1 }),
    ] {
        let (status, refused) = put_planes(app, "production", body).await;
        assert_eq!(status, StatusCode::CONFLICT, "{refused}");
        assert_eq!(refused["error"], "production_has_no_planes");
    }
    let (status, refused) = send(
        "POST",
        &format!("{app}/environments/production/reset"),
        Some(json!({ "planes": ["storage"] })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["error"], "production_has_no_planes");
    assert_eq!(
        audit_rows(&t.db, app, "app.environment.planes_set").await,
        0
    );
}

/// A choice that does not parse is `400 invalid_planes` and leaves the row
/// as it was; an unknown sandbox is `404`; staging with no row is `409`.
#[tokio::test]
async fn a_bad_choice_a_missing_sandbox_and_staging_with_no_row_are_refused() {
    let t = seeded_tenant().await;
    let app = staffed_app(&t).await;
    create(app, json!({ "name": "dev-a1" })).await;
    let kept = json!({ "workspace-actions": "held" });
    put_planes(app, "dev-a1", kept.clone()).await;

    for bad in [
        json!({ "olpt": "schema" }),
        json!({ "oltp": "teleport" }),
        json!(["oltp"]),
        json!("oltp"),
        json!(null),
    ] {
        let (status, refused) = put_planes(app, "dev-a1", bad.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {refused}");
        assert_eq!(refused["error"], "invalid_planes", "{bad}");
    }
    assert_eq!(stored_planes(&t.db, app, "dev-a1").await, Some(kept));

    let (status, missing) = put_planes(app, "dev-none", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");
    assert_eq!(missing["error"], "environment_not_found");
    let (status, invalid) = put_planes(app, "nope", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{invalid}");
    assert_eq!(invalid["error"], "invalid_environment_name");

    // Staging: a row it has takes the choice; one it does not is not made.
    let (status, set) = put_planes(app, "staging", json!({ "workspace-actions": "refused" })).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "staging has a row once published: {set}"
    );
    assert_eq!(set["planes"]["workspace-actions"], "refused");
    t.db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "DELETE FROM app_environments WHERE app_id = $1 AND name = 'staging'",
        [app.into()],
    ))
    .await
    .expect("drop staging's row");
    let (status, no_row) = put_planes(app, "staging", json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{no_row}");
    assert_eq!(no_row["error"], "environment_has_no_row");
    assert_eq!(stored_planes(&t.db, app, "staging").await, None);
}

/// A create records what it names: the parent, the fork point, and the planes
/// (the preset under the explicit keys). `from` must be a real parent and
/// `at` a past time; a refused create leaves no sandbox.
#[tokio::test]
async fn a_create_records_from_at_and_planes() {
    let t = seeded_tenant().await;
    let app = staffed_app(&t).await;

    let (status, plain) = create(app, json!({ "name": "dev-p1" })).await;
    assert_eq!(status, StatusCode::CREATED, "{plain}");
    assert_eq!(
        plain["parent"], "staging",
        "the default parent of a sandbox"
    );

    let (status, first) = create(
        app,
        json!({
            "name": "dev-b2",
            "from": "production",
            "at": "2026-01-02T03:04:05Z",
            "preset": "full",
            // `full` names `oltp: branch`, which needs an org with an OLTP
            // database (`env_oltp_branch::planes`); the explicit key wins.
            "planes": { "oltp": "schema", "workspace-actions": "held" },
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    assert_eq!(first["parent"], "production");
    assert_eq!(first["fork_at"], "2026-01-02T03:04:05Z");
    assert_eq!(
        first["planes_set"],
        json!({ "oltp": "schema", "airhouse": "branch", "secrets": "empty", "messages": "capture", "workspace-actions": "held" }),
        "the full preset under the explicit keys"
    );
    assert_eq!(first["planes"]["airhouse"], "branch");
    assert_eq!(first["planes"]["messages"], "capture");
    assert_eq!(first["planes"]["workspace-actions"], "held");
    assert_eq!(first["planes"]["secrets"], "empty");

    let (status, forked) = create(app, json!({ "name": "dev-c3", "from": "dev-b2" })).await;
    assert_eq!(status, StatusCode::CREATED, "{forked}");
    assert_eq!(forked["parent"], "dev-b2");

    let (status, light) = create(app, json!({ "name": "dev-d4", "preset": "light" })).await;
    assert_eq!(status, StatusCode::CREATED, "{light}");
    assert_eq!(light["planes_set"], json!({}));

    for (body, code) in [
        (
            json!({ "name": "dev-e5", "from": "dev-nobody" }),
            "invalid_parent",
        ),
        (
            json!({ "name": "dev-e5", "from": "nope" }),
            "invalid_parent",
        ),
        (
            json!({ "name": "dev-e5", "at": "yesterday" }),
            "invalid_fork_at",
        ),
        (
            json!({ "name": "dev-e5", "at": "2999-01-01T00:00:00Z" }),
            "invalid_fork_at",
        ),
        (
            json!({ "name": "dev-e5", "preset": "heavy" }),
            "invalid_planes",
        ),
        (
            json!({ "name": "dev-e5", "planes": { "olpt": "schema" } }),
            "invalid_planes",
        ),
        (
            json!({ "name": "dev-e5", "planes": "oltp" }),
            "invalid_planes",
        ),
    ] {
        let (status, refused) = create(app, body.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {refused}");
        assert_eq!(refused["error"], code, "{body}");
    }
    assert_eq!(stored_planes(&t.db, app, "dev-e5").await, None);
}

/// The reset route validates, then refuses a plane with no reset
/// (`sandbox_reset` has the planes that do). Airhouse has one: on a sandbox
/// whose overlay holds nothing it is done without touching Airhouse.
#[tokio::test]
async fn reset_refuses_a_plane_with_no_reset() {
    let t = seeded_tenant().await;
    let app = staffed_app(&t).await;
    create(app, json!({ "name": "dev-a1" })).await;
    for bad in [json!([]), json!(["storege"]), json!(["storage", "nope"])] {
        let (status, refused) = reset(app, bad.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {refused}");
        assert_eq!(refused["error"], "invalid_planes", "{bad}");
    }
    for plane in ["oltp", "workspace-actions"] {
        let (status, refused) = reset(app, json!([plane])).await;
        assert_eq!(status, StatusCode::CONFLICT, "{plane}: {refused}");
        assert_eq!(refused["error"], "plane_not_resettable", "{plane}");
        assert!(
            refused["message"]
                .as_str()
                .is_some_and(|m| m.contains(plane))
        );
    }
    let (status, done) = reset(app, json!(["airhouse"])).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(
        done["planes"],
        json!([{ "plane": "airhouse", "status": "done", "run_id": null }])
    );
}

/// Airhouse's overlay modes copy production rows, so they are for sandboxes
/// only: staging refuses them with the reason, and keeps its planes.
#[tokio::test]
async fn staging_refuses_the_airhouse_overlay() {
    let t = seeded_tenant().await;
    let app = staffed_app(&t).await;
    let before = stored_planes(&t.db, app, "staging").await;
    for mode in ["branch", "snapshot"] {
        let (status, refused) = put_planes(app, "staging", json!({ "airhouse": mode })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{mode}: {refused}");
        assert_eq!(refused["error"], "invalid_planes", "{mode}");
        assert!(
            refused["message"]
                .as_str()
                .is_some_and(|m| m.contains("sandboxes only")),
            "{refused}"
        );
    }
    assert_eq!(stored_planes(&t.db, app, "staging").await, before);
}

/// A `snapshot` sandbox's fork point is checked when it is cut, where `--at`
/// is the only thing that names it. An existing sandbox's is not: its pin is
/// checked where it is read, and the reset re-pins it, so an old fork point
/// never stops its planes being saved.
#[tokio::test]
async fn only_a_create_checks_the_snapshot_fork_point() {
    let t = seeded_tenant().await;
    let app = staffed_app(&t).await;
    let old = json!({
        "name": "dev-g7",
        "from": "production",
        "at": "2026-01-02T03:04:05Z",
        "planes": { "airhouse": "snapshot" },
    });
    let (status, refused) = create(app, old).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["error"], "invalid_planes");

    let (status, cut) = create(
        app,
        json!({ "name": "dev-g7", "from": "production", "at": "2026-01-02T03:04:05Z" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{cut}");
    for _ in 0..2 {
        let (status, saved) = put_planes(app, "dev-g7", json!({ "airhouse": "snapshot" })).await;
        assert_eq!(status, StatusCode::OK, "{saved}");
    }
}
