//! A custom app's failed production check (`oxy.production_check_failed`),
//! against real rows: the notice is worded from the check's row, its app and
//! its build when the inbox is read, reaches push, email and Slack, and is
//! told once per check.

use entity::{app_build_checks, app_builds, apps};
use oxy_api_notifications::platform::kinds::PRODUCTION_CHECK_FAILED;
use oxy_api_notifications::platform::{PlatformNotice, notify};
use sea_orm::ActiveModelTrait;
use sea_orm::ActiveValue::Set;

use super::*;

async fn seed_app(db: &DatabaseConnection, org: Uuid, workspace: Uuid) -> apps::Model {
    apps::ActiveModel {
        id: Set(Uuid::new_v4()),
        slug: Set("store-ops".into()),
        name: Set("Store Ops".into()),
        org_id: Set(org),
        project_id: Set(workspace),
        branch: Set("main".into()),
        source_repo: Set("acme/store-ops".into()),
        status: Set("active".into()),
        source_type: Set("s3".into()),
        source_config: Set(json!({})),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed app")
}

async fn seed_build(db: &DatabaseConnection, app: Uuid, build_id: &str) -> app_builds::Model {
    app_builds::ActiveModel {
        id: Set(Uuid::new_v4()),
        app_id: Set(app),
        build_id: Set(build_id.into()),
        s3_prefix: Set(format!("customer-apps/{app}/builds/{build_id}/")),
        manifest_json: Set(None),
        created_at: Set(chrono::Utc::now().fixed_offset()),
        validation_status: Set("passed".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed build")
}

/// A failed `production_smoke` row of `build`, as the runner's report and the
/// platform's follow-up leave it.
async fn seed_check(
    db: &DatabaseConnection,
    (app, build): (Uuid, Uuid),
    detail: serde_json::Value,
) -> Uuid {
    let id = Uuid::new_v4();
    let now = chrono::Utc::now().fixed_offset();
    app_build_checks::ActiveModel {
        id: Set(id),
        app_id: Set(app),
        build_id: Set(build),
        environment: Set("production".into()),
        kind: Set("production_smoke".into()),
        attempt: Set(1),
        status: Set("failed".into()),
        advisory: Set(false),
        summary: Set(Some("1 new error on 1 of 2 screens".into())),
        detail: Set(Some(detail)),
        notes: Set(json!([])),
        run_id: Set(None),
        runner: Set(None),
        created_at: Set(now),
        started_at: Set(None),
        finished_at: Set(Some(now)),
    }
    .insert(db)
    .await
    .expect("seed check");
    id
}

#[tokio::test]
async fn platform_notices_a_failed_production_check_is_worded_from_its_row() {
    let (db, _url) = fresh_db(Schema::All).await;
    let org = seed_org(&db).await;
    let ws = seed_workspace(&db, org.id, "Ops", WorkspaceStatus::Ready, None).await;
    let app = seed_app(&db, org.id, ws).await;
    let before = seed_build(&db, app.id, "b-41").await;
    let live = seed_build(&db, app.id, "b-42").await;
    let detail = json!({
        "findings": [],
        "new_errors": [{ "path": "stores", "viewport": "desk", "error": "TypeError: x" }],
        "move": { "action": "promote", "by": org.admin, "at": "2026-11-22T10:00:00Z",
            "previous": { "id": before.id, "build_id": "b-41" } },
        "rollback": { "rolled_back": true, "to": { "id": before.id, "build_id": "b-41" },
            "at": "2026-11-22T10:05:00Z" },
    });
    let check = seed_check(&db, (app.id, live.id), detail).await;

    let notice = PlatformNotice::ProductionCheckFailed {
        org_id: org.id,
        workspace_id: ws,
        check_id: check,
    };
    let sent = notify(&db, &notice, &[org.admin, org.owner]).await.unwrap();
    assert_eq!(sent.recipients, 2);
    let rows = told(&db, PRODUCTION_CHECK_FAILED, &check.to_string()).await;
    assert_eq!(people(&rows), sorted(vec![org.admin, org.owner]));
    let send = notification_sends::Entity::find_by_id(sent.id.unwrap())
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert!(send.email && send.slack && !send.critical, "every channel");

    let items = inbox_view::items(&db, rows).await.expect("inbox");
    assert_eq!(
        items[0].title.as_deref(),
        Some("Store Ops: new errors on production after build b-42")
    );
    assert_eq!(
        items[0].body.as_deref(),
        Some(
            "Build b-42 showed 1 new error on production since it replaced build b-41. \
             /stores at desk: TypeError: x. Production was rolled back to build b-41 \
             automatically."
        )
    );
    assert_eq!(
        items[0].href.as_deref(),
        Some(format!("/admin/apps/{}/store-ops/releases", org.slug).as_str())
    );

    // The same check again tells nobody twice.
    let again = notify(&db, &notice, &[org.admin, org.owner]).await.unwrap();
    assert_eq!((again.recipients, again.already_told), (0, 2));

    // The check's row gone (its build pruned): the plain sentence, no link.
    app_builds::Entity::delete_by_id(live.id)
        .exec(&db)
        .await
        .unwrap();
    let rows = told(&db, PRODUCTION_CHECK_FAILED, &check.to_string()).await;
    let items = inbox_view::items(&db, rows).await.expect("inbox");
    assert_eq!(
        items[0].title.as_deref(),
        Some("An app shows new errors on production")
    );
    assert_eq!(items[0].href, None);
}

/// A check whose attempt had no baseline from before the move, as the
/// follow-up leaves its row (`move.baseline: false`, and the rollback it did
/// not make): the inbox says errors on production, never new ones.
#[tokio::test]
async fn platform_notices_a_failed_production_check_with_no_baseline_calls_nothing_new() {
    let (db, _url) = fresh_db(Schema::All).await;
    let org = seed_org(&db).await;
    let ws = seed_workspace(&db, org.id, "Ops", WorkspaceStatus::Ready, None).await;
    let app = seed_app(&db, org.id, ws).await;
    let before = seed_build(&db, app.id, "b-41").await;
    let live = seed_build(&db, app.id, "b-42").await;
    let detail = json!({
        "findings": [], "baseline_at": null,
        "new_errors": [{ "path": "stores", "viewport": "desk", "error": "TypeError: x" }],
        "move": { "action": "publish", "by": org.admin, "at": "2026-11-22T10:00:00Z",
            "previous": { "id": before.id, "build_id": "b-41" }, "baseline": false },
        "rollback": { "rolled_back": false,
            "reason": "production had no baseline from before the move to compare its errors with",
            "at": "2026-11-22T10:05:00Z" },
    });
    let check = seed_check(&db, (app.id, live.id), detail).await;
    let notice = PlatformNotice::ProductionCheckFailed {
        org_id: org.id,
        workspace_id: ws,
        check_id: check,
    };
    let sent = notify(&db, &notice, &[org.admin]).await.unwrap();
    assert_eq!(sent.recipients, 1);

    let rows = told(&db, PRODUCTION_CHECK_FAILED, &check.to_string()).await;
    let items = inbox_view::items(&db, rows).await.expect("inbox");
    assert_eq!(
        items[0].title.as_deref(),
        Some("Store Ops: errors on production after build b-42")
    );
    assert_eq!(
        items[0].body.as_deref(),
        Some(
            "Build b-42 is live and production shows 1 error since it replaced build b-41. \
             /stores at desk: TypeError: x. Production was not rolled back automatically: \
             production had no baseline from before the move to compare its errors with."
        )
    );
}
