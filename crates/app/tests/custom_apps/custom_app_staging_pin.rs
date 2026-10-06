//! Custom-app staging, semantic half (`internal-docs/customer-apps-staging.md`
//! D4): a draft build pins a `staging` revision, and only a staging request —
//! the app's staging host, from a caller who may open staging — reads it. The
//! retired `oxy_preview_draft` cookie pins nothing.
//!
//! The workspace has a promoted `main` revision carrying view `orders` at
//! definition A, and a `staging` revision with the same view at definition B.
//! The app's live build has no pin; its draft build pins B. Revision rows are
//! inserted directly — the compile itself is `oxy-compile`'s to test.

use crate::common::test_db;
use axum::http::{HeaderMap, HeaderValue};
use entity::{app_admins, app_builds, apps, organizations, revisions, semantic_views, workspaces};
use oxy_app::server::api::compiled_reader::{resolve_request_revision, resolve_semantic_view};
use oxy_app::server::api::custom_apps_staging_pin::{
    APP_HEADER, PinRefusal, pin_drift, pinned_revision_for, staging_pin_for_data_request,
    validate_pin_for_publish, with_staging_pin,
};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    EntityTrait, Statement,
};
use serde_json::json;
use uuid::Uuid;

const VIEW_PATH: &str = "semantics/views/orders.view.yml";
const STAFF: &str = "staging-staff@example.com";

struct World {
    workspace: Uuid,
    app: Uuid,
    promoted: Uuid,
    staging: Uuid,
    live_build: Uuid,
    draft_build: Uuid,
}

async fn seed_revision(
    db: &DatabaseConnection,
    workspace: Uuid,
    kind: &str,
    sha: &str,
    table: &str,
) -> Uuid {
    let now = chrono::Utc::now().fixed_offset();
    let id = Uuid::new_v4();
    revisions::ActiveModel {
        revision_id: ActiveValue::Set(id),
        workspace_id: ActiveValue::Set(workspace),
        git_sha: ActiveValue::Set(sha.into()),
        branch: ActiveValue::Set(Some(if kind == "main" { "main" } else { "feat" }.into())),
        // The compiler's own version, not a literal: reuse is keyed on it
        // (`find_reusable_revision`), so a hard-coded `1` stopped being
        // reusable the day the version moved.
        schema_version: ActiveValue::Set(oxy_compile::CURRENT_SCHEMA_VERSION),
        status: ActiveValue::Set("ready".into()),
        kind: ActiveValue::Set(kind.into()),
        owner_user_id: ActiveValue::Set(None),
        compiler_version: ActiveValue::Set("test".into()),
        started_at: ActiveValue::Set(now),
        finished_at: ActiveValue::Set(Some(now)),
        file_count_seen: ActiveValue::Set(1),
        file_count_compiled: ActiveValue::Set(1),
        file_count_failed: ActiveValue::Set(0),
        error_summary: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed revision");
    semantic_views::ActiveModel {
        revision_id: ActiveValue::Set(id),
        name: ActiveValue::Set("orders".into()),
        file_path: ActiveValue::Set(VIEW_PATH.into()),
        definition: ActiveValue::Set(json!({ "name": "orders", "table": table })),
        compiled_sql_blob_key: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed view");
    id
}

async fn seed_build(db: &DatabaseConnection, app: Uuid, pin: Option<Uuid>) -> Uuid {
    let id = Uuid::new_v4();
    app_builds::ActiveModel {
        id: ActiveValue::Set(id),
        app_id: ActiveValue::Set(app),
        build_id: ActiveValue::Set(format!("b-{}", id.simple())),
        s3_prefix: ActiveValue::Set(format!("customer-apps/{app}/builds/{id}/")),
        created_at: ActiveValue::Set(chrono::Utc::now().into()),
        validation_status: ActiveValue::Set("passed".into()),
        semantic_revision_id: ActiveValue::Set(pin),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed build");
    id
}

async fn seed_org_workspace(db: &DatabaseConnection) -> (Uuid, Uuid) {
    let org = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org),
        name: ActiveValue::Set("Staging Org".into()),
        slug: ActiveValue::Set(format!("stg-{}", org.simple())),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed org");
    let workspace = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(workspace),
        name: ActiveValue::Set("Staging Workspace".into()),
        org_id: ActiveValue::Set(Some(org)),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed workspace");
    (org, workspace)
}

async fn promote(db: &DatabaseConnection, workspace: Uuid, revision: Uuid) {
    let mut ws: workspaces::ActiveModel = workspaces::Entity::find_by_id(workspace)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .into();
    ws.current_revision_id = ActiveValue::Set(Some(revision));
    ws.update(db).await.expect("promote");
}

async fn seed_world(db: &DatabaseConnection) -> World {
    let (org, workspace) = seed_org_workspace(db).await;
    let promoted = seed_revision(db, workspace, "main", "sha-main", "orders_a").await;
    promote(db, workspace, promoted).await;
    let staging = seed_revision(db, workspace, "staging", "sha-feat", "orders_b").await;

    let app = Uuid::new_v4();
    apps::ActiveModel {
        id: ActiveValue::Set(app),
        slug: ActiveValue::Set(format!("stg-app-{}", &app.simple().to_string()[..8])),
        name: ActiveValue::Set("Staging App".into()),
        org_id: ActiveValue::Set(org),
        project_id: ActiveValue::Set(workspace),
        branch: ActiveValue::Set("main".into()),
        source_repo: ActiveValue::Set("stg/test".into()),
        status: ActiveValue::Set("active".into()),
        source_type: ActiveValue::Set("s3".into()),
        source_config: ActiveValue::Set(json!({})),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed app");
    let live_build = seed_build(db, app, None).await;
    let draft_build = seed_build(db, app, Some(staging)).await;
    let mut row: apps::ActiveModel = apps::Entity::find_by_id(app)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .into();
    row.published_build_id = ActiveValue::Set(Some(live_build));
    row.draft_build_id = ActiveValue::Set(Some(draft_build));
    row.published_at = ActiveValue::Set(Some(chrono::Utc::now().into()));
    row.update(db).await.expect("point builds");

    // Platform standing that reaches every org with `develop_apps`.
    let now = chrono::Utc::now().fixed_offset();
    app_admins::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        email: ActiveValue::Set(STAFF.into()),
        granted_by: ActiveValue::Set(None),
        created_at: ActiveValue::Set(now),
        role: ActiveValue::Set("app_operator".into()),
        scope_all: ActiveValue::Set(true),
        updated_at: ActiveValue::Set(now),
    }
    .insert(db)
    .await
    .expect("seed staff grant");

    World {
        workspace,
        app,
        promoted,
        staging,
        live_build,
        draft_build,
    }
}

fn staging_headers(app: Uuid, cookie: bool) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(APP_HEADER, HeaderValue::from_str(&app.to_string()).unwrap());
    if cookie {
        // The retired staff draft-preview cookie. Sent on purpose by the tests
        // that prove it no longer makes a request a staging one.
        h.insert(
            "cookie",
            HeaderValue::from_static("oxy_session=x; oxy_preview_draft=1"),
        );
    }
    h
}

/// A data request on `host`, naming the app by header.
fn on_host(app: Uuid, host: &str, cookie: bool) -> HeaderMap {
    let mut h = staging_headers(app, cookie);
    h.insert("host", HeaderValue::from_str(host).unwrap());
    h
}

/// The app's staging host. Shortens the org slug so the label fits.
async fn staging_host_of(db: &DatabaseConnection, app: Uuid) -> String {
    let (org_slug, app_slug) = short_slugs(db, app).await;
    format!("staging--{org_slug}--{app_slug}.customer-apps.oxygen-hq.com")
}

async fn view_table(workspace: Uuid) -> String {
    let view = resolve_semantic_view(workspace, None, VIEW_PATH)
        .await
        .expect("view query")
        .expect("view resolves from the boundary");
    view.definition["table"].as_str().unwrap().to_string()
}

async fn current_revision(db: &DatabaseConnection, workspace: Uuid) -> Option<Uuid> {
    workspaces::Entity::find_by_id(workspace)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .current_revision_id
}

/// The acceptance bar: staging reads B, everything else reads A, and nothing
/// moved `current_revision_id`.
#[tokio::test]
async fn a_staging_request_reads_the_pinned_revision_and_live_reads_the_promoted_one() {
    let db = test_db().await;
    let w = seed_world(&db).await;

    let host = staging_host_of(&db, w.app).await;
    let pin = staging_pin_for_data_request(
        &db,
        &on_host(w.app, &host, false),
        &oxy_app::server::authz::Caller::without_credential(Uuid::new_v4(), STAFF),
        w.workspace,
    )
    .await;
    assert_eq!(
        pin,
        Some(w.staging),
        "a staging request resolves the draft's pin"
    );

    // Every compile-boundary read inside the staging request answers from B —
    // including the request-entry resolution `build_project_context` records.
    let (resolved, table) = with_staging_pin(pin, async {
        (
            resolve_request_revision(w.workspace, None).await,
            view_table(w.workspace).await,
        )
    })
    .await;
    assert_eq!(resolved, Some(w.staging));
    assert_eq!(table, "orders_b");

    // An unrelated request (no pin scope) reads A.
    assert_eq!(view_table(w.workspace).await, "orders_a");
    assert_eq!(
        resolve_request_revision(w.workspace, None).await,
        Some(w.promoted)
    );

    // The live build carries no pin, so a live invocation scopes nothing.
    assert_eq!(pinned_revision_for(&db, w.live_build).await, None);
    let live = with_staging_pin(
        pinned_revision_for(&db, w.live_build).await,
        view_table(w.workspace),
    )
    .await;
    assert_eq!(live, "orders_a");
    // The draft build's pin is what the function invocation will use.
    assert_eq!(
        pinned_revision_for(&db, w.draft_build).await,
        Some(w.staging)
    );

    assert_eq!(current_revision(&db, w.workspace).await, Some(w.promoted));
}

/// Each of the gates is required on its own, and the retired preview cookie is
/// not one of them: on the production host it pins nothing, even for staff.
#[tokio::test]
async fn the_preview_cookie_no_reach_or_another_workspace_means_no_pin() {
    let db = test_db().await;
    let w = seed_world(&db).await;
    let host = staging_host_of(&db, w.app).await;

    for headers in [staging_headers(w.app, false), staging_headers(w.app, true)] {
        let pin = staging_pin_for_data_request(
            &db,
            &headers,
            &oxy_app::server::authz::Caller::without_credential(Uuid::new_v4(), STAFF),
            w.workspace,
        )
        .await;
        assert_eq!(
            pin, None,
            "off the staging host, with or without oxy_preview_draft, reads the live revision"
        );
    }

    let member = staging_pin_for_data_request(
        &db,
        &on_host(w.app, &host, true),
        &oxy_app::server::authz::Caller::without_credential(Uuid::new_v4(), "customer@example.com"),
        w.workspace,
    )
    .await;
    assert_eq!(
        member, None,
        "staging without DevelopApps reach pins nothing"
    );

    let (_, other_ws) = seed_org_workspace(&db).await;
    let elsewhere = staging_pin_for_data_request(
        &db,
        &on_host(w.app, &host, false),
        &oxy_app::server::authz::Caller::without_credential(Uuid::new_v4(), STAFF),
        other_ws,
    )
    .await;
    assert_eq!(
        elsewhere, None,
        "the named app must be published from the request's workspace"
    );
}

/// A data request on the app's staging host (environments design §3.2) is a
/// staging request with no cookie at all: the pin comes from the build the
/// staging environment serves, for a viewer who may open staging.
#[tokio::test]
async fn a_staging_host_request_reads_the_staging_builds_pin() {
    let db = test_db().await;
    let w = seed_world(&db).await;
    let (org_slug, app_slug) = short_slugs(&db, w.app).await;
    let on = |host: &str| on_host(w.app, host, false);
    let staging_host = format!("staging--{org_slug}--{app_slug}.customer-apps.oxygen-hq.com");
    let production_host = format!("{org_slug}--{app_slug}.customer-apps.oxygen-hq.com");

    let staff = staging_pin_for_data_request(
        &db,
        &on(&staging_host),
        &oxy_app::server::authz::Caller::without_credential(Uuid::new_v4(), STAFF),
        w.workspace,
    )
    .await;
    assert_eq!(staff, Some(w.staging), "the staging build's pin, no cookie");

    let customer = staging_pin_for_data_request(
        &db,
        &on(&staging_host),
        &oxy_app::server::authz::Caller::without_credential(Uuid::new_v4(), "customer@example.com"),
        w.workspace,
    )
    .await;
    assert_eq!(customer, None, "staging is Oxy staff's");

    let production = staging_pin_for_data_request(
        &db,
        &on(&production_host),
        &oxy_app::server::authz::Caller::without_credential(Uuid::new_v4(), STAFF),
        w.workspace,
    )
    .await;
    assert_eq!(production, None, "production reads the promoted revision");
}

/// Slugs short enough for a `staging--<org>--<slug>` DNS label (63 bytes).
async fn short_slugs(db: &DatabaseConnection, app: Uuid) -> (String, String) {
    let app_row = apps::Entity::find_by_id(app)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let mut org: organizations::ActiveModel = organizations::Entity::find_by_id(app_row.org_id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .into();
    let org_slug = format!("stg{}", &app.simple().to_string()[..6]);
    org.slug = ActiveValue::Set(org_slug.clone());
    org.update(db).await.expect("shorten the org slug");
    (org_slug, app_row.slug)
}

#[tokio::test]
async fn a_staging_revision_is_never_promoted_and_does_not_supersede_main() {
    let db = test_db().await;
    let w = seed_world(&db).await;

    // Rollback-to-prior refuses a staging revision.
    let err = oxy_compile::promote_existing(&db, w.workspace, w.staging).await;
    assert!(err.is_err(), "promote_existing must refuse kind=staging");
    assert_eq!(current_revision(&db, w.workspace).await, Some(w.promoted));

    // `idx_revisions_idempotent_ready_main` is main-only: a later ready MAIN
    // revision of the staging revision's SHA inserts rather than colliding.
    let later_main = seed_revision(&db, w.workspace, "main", "sha-feat", "orders_b").await;
    assert_ne!(later_main, w.staging);

    // Reuse: a staging compile of the SHA reuses the staging row; a main
    // compile never does (its promote would land on a branch head).
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE revisions SET compiler_version = $1 WHERE revision_id = $2",
        [oxy_compile::compiler_version().into(), w.staging.into()],
    ))
    .await
    .unwrap();
    use oxy_compile::{RevisionKind, find_reusable_revision};
    let as_main = find_reusable_revision(&db, w.workspace, RevisionKind::Main, "sha-feat").await;
    assert_eq!(as_main.unwrap(), None);
    let as_staging =
        find_reusable_revision(&db, w.workspace, RevisionKind::Staging, "sha-feat").await;
    assert_eq!(as_staging.unwrap(), Some(w.staging));
}

#[tokio::test]
async fn retention_keeps_a_revision_a_build_pins() {
    let db = test_db().await;
    let w = seed_world(&db).await;
    let orphan = seed_revision(&db, w.workspace, "staging", "sha-old", "orders_c").await;
    db.execute_raw(Statement::from_string(
        DatabaseBackend::Postgres,
        "UPDATE revisions SET finished_at = now() - interval '30 days'".to_string(),
    ))
    .await
    .unwrap();

    oxy_app::server::compile_maintenance::prune_old_revisions(&db, 7).await;

    let exists = |id: Uuid| {
        let db = db.clone();
        async move {
            revisions::Entity::find_by_id(id)
                .one(&db)
                .await
                .unwrap()
                .is_some()
        }
    };
    assert!(exists(w.staging).await, "pinned by the draft build");
    assert!(exists(w.promoted).await, "current");
    assert!(!exists(orphan).await, "unpinned and old");
}

#[tokio::test]
async fn publish_refuses_a_pin_with_promote_or_from_elsewhere() {
    let db = test_db().await;
    let w = seed_world(&db).await;

    assert_eq!(
        validate_pin_for_publish(&db, w.workspace, w.staging, false).await,
        Ok(())
    );
    assert_eq!(
        validate_pin_for_publish(&db, w.workspace, w.staging, true).await,
        Err(PinRefusal::WithPromote)
    );
    let (_, other_ws) = seed_org_workspace(&db).await;
    assert!(matches!(
        validate_pin_for_publish(&db, other_ws, w.staging, false).await,
        Err(PinRefusal::OtherWorkspace { .. })
    ));
}

#[tokio::test]
async fn promoting_a_pinned_build_names_the_views_that_differ() {
    let db = test_db().await;
    let w = seed_world(&db).await;

    assert_eq!(
        pin_drift(&db, w.live_build).await,
        None,
        "no pin, no notice"
    );
    let notice = pin_drift(&db, w.draft_build).await.expect("pinned build");
    assert_eq!(notice.pinned_revision_id, w.staging);
    assert_eq!(notice.current_revision_id, Some(w.promoted));
    assert_eq!(notice.views_differ, vec!["orders".to_string()]);
    assert!(notice.topics_differ.is_empty());
}

// The function host exists only with the `custom-app-functions` feature.
#[cfg(feature = "custom-app-functions")]
mod airway_host;

/// I9: a staging (or preview) invocation cannot start production ELT. Refused
/// for a host built under a staging pin — then called outside it, as the
/// isolate thread calls it — and for a host whose context reads a `staging`
/// revision. The control, a live host, gets past the refusal and fails on the
/// pipeline it cannot find, which is what makes the refusals mean anything.
///
/// The pinned run is a preview, so it holds every other write as staging does
/// (a `POST` answers 409 unsent) — and, being production-admitted, logs no
/// `app.staging.held` row for any of it.
#[cfg(feature = "custom-app-functions")]
#[tokio::test]
async fn ctx_airway_run_is_refused_under_a_staging_pin() {
    const REFUSED: &str = "isn't available in a staging or preview invocation";
    let db = test_db().await;
    let w = seed_world(&db).await;
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO workspace_compiled_configs (revision_id, databases) VALUES ($1, '[]'::jsonb)",
        [w.staging.into()],
    ))
    .await
    .expect("staging config");
    let root = tempfile::tempdir().expect("workspace dir");
    std::fs::write(
        root.path().join("config.yml"),
        "databases: []\nmodels: []\n",
    )
    .unwrap();
    let run = |host: std::sync::Arc<
        dyn oxy_app::server::api::custom_apps_functions::runtime::FunctionHost,
    >| async move {
        let err = host
            .airway_run("airway/orders.airway.yml".into(), serde_json::Value::Null)
            .await
            .expect_err("no call here seeds a run");
        // Where a held-write row would be written, were one due.
        host.end_of_invocation().await;
        err
    };

    let pinned = with_staging_pin(
        Some(w.staging),
        airway_host::host(&db, w.workspace, root.path(), None),
    )
    .await;
    // Held before anything is sent: nothing here reaches api.example.com.
    let posted = pinned
        .fetch(
            "https://api.example.com/hook".into(),
            json!({ "method": "POST", "body": "{}" }),
        )
        .await
        .expect("a held fetch resolves");
    assert_eq!(posted["status"], 409, "held as staging holds it: {posted}");
    let held_body = posted["body"].as_str().unwrap_or_default();
    assert!(
        held_body.contains("reads a branch") && held_body.contains(&w.staging.to_string()),
        "the hold names the branch read, not an environment: {held_body}"
    );
    let err = run(pinned).await;
    assert!(
        err.contains(REFUSED) && err.contains(&w.staging.to_string()),
        "{err}"
    );

    let at_staging = airway_host::host(&db, w.workspace, root.path(), Some(w.staging)).await;
    let err = run(at_staging).await;
    assert!(err.contains(REFUSED), "{err}");

    let live = airway_host::host(&db, w.workspace, root.path(), None).await;
    let err = run(live).await;
    assert!(
        !err.contains(REFUSED),
        "a live invocation is not refused: {err}"
    );

    // Each hold and refusal was a production-admitted run's: none is logged as
    // a held staging call, whose `environment` column would say `production`.
    use sea_orm::{ColumnTrait, QueryFilter};
    let held = entity::audit_events::Entity::find()
        .filter(entity::audit_events::Column::Action.eq("app.staging.held"))
        .all(&db)
        .await
        .expect("query audit_events");
    assert!(held.is_empty(), "no held row for a branch read: {held:?}");
}
