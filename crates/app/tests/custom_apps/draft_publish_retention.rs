//! A draft never spends a rollback slot: the builds production served are
//! kept in a retention window of their own (`custom_apps_sandboxes::retention`),
//! so no number of draft publishes prunes one production could roll back to —
//! and the drafts' window and the sandboxes' are what they were.

use entity::app_builds;
use oxy_app::server::api::custom_apps_sandboxes::ops;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::json;
use uuid::Uuid;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{Tenant, publish_build, seeded_tenant};
use crate::sandbox_publish::{app_row, build_pk, noop, publish_to_sandbox, sandbox};
use crate::staging_functions::make_guest_staff;

async fn labels(db: &DatabaseConnection, app_id: Uuid) -> Vec<String> {
    let mut labels: Vec<String> = app_builds::Entity::find()
        .filter(app_builds::Column::AppId.eq(app_id))
        .all(db)
        .await
        .expect("read the builds")
        .into_iter()
        .map(|build| build.build_id)
        .collect();
    labels.sort();
    labels
}

async fn publish(t: &Tenant, slug: &str, build_id: &str, promote: bool) -> Uuid {
    let route = [noop("noop", json!({ "route": true }))];
    publish_build(t, slug, demo_workspace_id(), build_id, promote, &route)
        .await
        .app_id
}

/// Three builds production ran, then twelve drafts. One shared window of ten
/// pruned `prod-1` and `prod-2`, the builds a rollback names; now all three
/// stay, and the drafts prune their own two oldest.
#[tokio::test]
async fn drafts_after_a_promote_leave_every_rollback_target() {
    let t = seeded_tenant().await;
    let slug = "draft-keep";
    let app_id = publish(&t, slug, "prod-1", true).await;
    publish(&t, slug, "prod-2", true).await;
    publish(&t, slug, "prod-3", true).await;
    for i in 1..=12 {
        publish(&t, slug, &format!("draft-{i:02}"), false).await;
    }

    let kept = labels(&t.db, app_id).await;
    for served in ["prod-1", "prod-2", "prod-3"] {
        assert!(kept.contains(&served.to_string()), "{served}: {kept:?}");
        // A rollback names a build row: each is still there to name.
        assert!(build_pk(&t.db, app_id, served).await.is_some(), "{served}");
    }
    for pruned in ["draft-01", "draft-02"] {
        assert!(!kept.contains(&pruned.to_string()), "{pruned}: {kept:?}");
    }
    assert!(kept.contains(&"draft-03".to_string()), "{kept:?}");
    assert!(kept.contains(&"draft-12".to_string()), "{kept:?}");
    assert_eq!(kept.len(), 13, "three served and ten drafts: {kept:?}");

    // Production still serves the last promote, and staging the last draft.
    let app = app_row(&t.db, app_id).await;
    assert_eq!(
        app.published_build_id,
        build_pk(&t.db, app_id, "prod-3").await
    );
    assert_eq!(
        app.draft_build_id,
        build_pk(&t.db, app_id, "draft-12").await
    );
}

/// The sandboxes' window is untouched by the new one: after a promote and ten
/// drafts, ten sandbox publishes prune nothing, and the eleventh prunes the
/// oldest sandbox build alone.
#[tokio::test]
async fn the_sandbox_window_is_what_it_was() {
    let t = seeded_tenant().await;
    let slug = "draft-sbx";
    let app_id = publish(&t, slug, "prod-1", true).await;
    publish(&t, slug, "prod-2", true).await;
    for i in 1..=10 {
        publish(&t, slug, &format!("draft-{i:02}"), false).await;
    }
    make_guest_staff();
    let app = app_row(&t.db, app_id).await;
    ops::create(&t.db, &app, &sandbox("a1"), &t.guest())
        .await
        .expect("create dev-a1");
    for i in 1..=10 {
        publish_to_sandbox(&t, slug, &format!("sbx-{i:02}"), "a1")
            .await
            .expect("publish to dev-a1");
    }
    let kept = labels(&t.db, app_id).await;
    assert_eq!(
        kept.len(),
        22,
        "two served, ten drafts, ten sandbox: {kept:?}"
    );

    publish_to_sandbox(&t, slug, "sbx-11", "a1")
        .await
        .expect("an eleventh publish to dev-a1");
    let kept = labels(&t.db, app_id).await;
    assert_eq!(kept.len(), 22, "{kept:?}");
    assert!(!kept.contains(&"sbx-01".to_string()), "{kept:?}");
    for stays in [
        "prod-1", "prod-2", "draft-01", "draft-10", "sbx-02", "sbx-11",
    ] {
        assert!(kept.contains(&stays.to_string()), "{stays}: {kept:?}");
    }
}
