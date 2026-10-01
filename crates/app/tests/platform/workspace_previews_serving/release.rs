//! Deleting a preview ends its staging revision's life as a preview, and deletes
//! the revision (with its compiled rows) unless something else still uses it:
//! another live preview at the same commit, an app build that pins it, a
//! preview run still reading it, or the workspace serving it. Either way the pin
//! stops honouring it — a revision that survives the delete is still no preview.
//!
//! Driven through `previews::service::delete` (what `DELETE /previews` runs)
//! and the real workspace middleware, over the serving fixture.

use entity::{app_builds, apps, workspace_preview_runs};
use oxy_app::server::previews::{namespace::preview_key, service};
use sea_orm::{ColumnTrait, PaginatorTrait, QueryFilter};

use super::*;

async fn served(fx: &Fx, rev: Uuid) -> Reply {
    call(&fx.staff, "GET", uri(fx, "probe"), Some(rev.to_string())).await
}

fn assert_served(r: &Reply, rev: Uuid, label: &str) {
    assert_eq!(r.body["which"], "staging", "{label}");
    assert_eq!(
        r.preview_header.as_deref(),
        Some(format!("{BRANCH}@{rev}").as_str()),
        "{label}"
    );
}

fn assert_not_served(r: &Reply, label: &str) {
    assert_eq!(r.status, StatusCode::OK, "{label}");
    assert_eq!(
        r.body["which"], "main",
        "{label}: answered as for an unknown revision"
    );
    assert_eq!(r.preview_header, None, "{label}: no x-oxy-preview stamp");
}

async fn exists(db: &DatabaseConnection, rev: Uuid) -> bool {
    revisions::Entity::find_by_id(rev)
        .one(db)
        .await
        .unwrap()
        .is_some()
}

async fn compiled_rows(db: &DatabaseConnection, rev: Uuid) -> u64 {
    semantic_views::Entity::find()
        .filter(semantic_views::Column::RevisionId.eq(rev))
        .count(db)
        .await
        .unwrap()
}

async fn delete(db: &DatabaseConnection, fx: &Fx, branch: &str) {
    service::delete(db, fx.ws, branch)
        .await
        .expect("delete preview");
}

/// A custom app of this workspace whose draft build pins `rev` — #3370's
/// staging pin, which retention never deletes out from under.
async fn pin_in_app_build(db: &DatabaseConnection, fx: &Fx, rev: Uuid) {
    let app = Uuid::new_v4();
    apps::ActiveModel {
        id: ActiveValue::Set(app),
        slug: ActiveValue::Set(format!("app-{}", &app.simple().to_string()[..8])),
        name: ActiveValue::Set("App".into()),
        org_id: ActiveValue::Set(fx.org),
        project_id: ActiveValue::Set(fx.ws),
        branch: ActiveValue::Set("main".into()),
        source_repo: ActiveValue::Set("acme/app".into()),
        status: ActiveValue::Set("active".into()),
        source_type: ActiveValue::Set("s3".into()),
        source_config: ActiveValue::Set(json!({})),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed app");
    let id = Uuid::new_v4();
    app_builds::ActiveModel {
        id: ActiveValue::Set(id),
        app_id: ActiveValue::Set(app),
        build_id: ActiveValue::Set(format!("b-{}", id.simple())),
        s3_prefix: ActiveValue::Set(format!("customer-apps/{app}/builds/{id}/")),
        created_at: ActiveValue::Set(chrono::Utc::now().into()),
        validation_status: ActiveValue::Set("passed".into()),
        semantic_revision_id: ActiveValue::Set(Some(rev)),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed build");
}

#[tokio::test]
async fn deleting_a_preview_stops_serving_its_revision_and_deletes_it() {
    let (db, fx) = setup().await;
    assert_served(&served(&fx, fx.staging_rev).await, fx.staging_rev, "live");
    assert!(compiled_rows(&db, fx.staging_rev).await > 0);

    delete(&db, &fx, BRANCH).await;

    assert_not_served(&served(&fx, fx.staging_rev).await, "after delete");
    assert!(
        !exists(&db, fx.staging_rev).await,
        "the preview's staging revision is deleted with it"
    );
    assert_eq!(
        compiled_rows(&db, fx.staging_rev).await,
        0,
        "and its compiled rows with it (the FK cascade)"
    );
    delete(&db, &fx, BRANCH).await; // idempotent
}

#[tokio::test]
async fn a_revision_an_app_build_pins_survives_the_delete_but_is_no_longer_a_preview() {
    let (db, fx) = setup().await;
    pin_in_app_build(&db, &fx, fx.staging_rev).await;

    delete(&db, &fx, BRANCH).await;

    assert!(
        exists(&db, fx.staging_rev).await,
        "never delete a revision a build pins"
    );
    assert_not_served(
        &served(&fx, fx.staging_rev).await,
        "a surviving revision is still not a preview once no preview is at it",
    );
}

#[tokio::test]
async fn a_revision_another_live_preview_is_at_survives_and_stays_served() {
    let (db, fx) = setup().await;
    // A second branch at the same commit: the staging compile reused the one
    // revision for both.
    preview(&db, fx.ws, "feat/y", fx.staging_rev, fx.staff.id).await;

    delete(&db, &fx, BRANCH).await;
    assert!(exists(&db, fx.staging_rev).await);
    assert_served(
        &served(&fx, fx.staging_rev).await,
        fx.staging_rev,
        "feat/y still previews it",
    );

    delete(&db, &fx, "feat/y").await;
    assert!(
        !exists(&db, fx.staging_rev).await,
        "the last preview took it"
    );
    assert_not_served(&served(&fx, fx.staging_rev).await, "both deleted");
}

#[tokio::test]
async fn a_revision_a_running_preview_run_reads_is_left_for_retention() {
    let (db, fx) = setup().await;
    workspace_preview_runs::ActiveModel {
        run_id: ActiveValue::Set(format!("run-{}", Uuid::new_v4().simple())),
        workspace_id: ActiveValue::Set(fx.ws),
        branch: ActiveValue::Set(BRANCH.into()),
        preview_key: ActiveValue::Set(preview_key(fx.ws, BRANCH)),
        revision_id: ActiveValue::Set(fx.staging_rev),
        kind: ActiveValue::Set("procedure".into()),
        target_ref: ActiveValue::Set(None),
        parent_run_id: ActiveValue::Set(None),
        options: ActiveValue::Set(json!({})),
        state: ActiveValue::Set("running".into()),
        requested_by: ActiveValue::Set(Some(fx.staff.id)),
        created_at: ActiveValue::Set(chrono::Utc::now().fixed_offset()),
        started_at: ActiveValue::Set(Some(chrono::Utc::now().fixed_offset())),
        finished_at: ActiveValue::Set(None),
    }
    .insert(&db)
    .await
    .expect("seed running run");

    delete(&db, &fx, BRANCH).await;

    assert!(
        exists(&db, fx.staging_rev).await,
        "a run in flight is not failed by deleting what it reads"
    );
    assert_not_served(&served(&fx, fx.staging_rev).await, "deleted preview");
}

#[tokio::test]
async fn a_preview_of_the_served_commit_never_deletes_the_workspaces_revision() {
    let (db, fx) = setup().await;
    let main_rev = workspaces::Entity::find_by_id(fx.ws)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .current_revision_id
        .expect("promoted");
    preview(&db, fx.ws, "feat/at-main", main_rev, fx.staff.id).await;

    delete(&db, &fx, "feat/at-main").await;

    assert!(
        exists(&db, main_rev).await,
        "main's revision is not a preview's"
    );
    let r = call(&fx.staff, "GET", uri(&fx, "probe"), None).await;
    assert_eq!(r.body["which"], "main");
}
