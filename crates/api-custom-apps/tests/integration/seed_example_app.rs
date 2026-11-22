//! `oxy dev seed` deploys the example custom app — end to end, against a real
//! Postgres and a real build store.
//!
//! The seed is a fixture the whole local-dev experience rests on, and every way
//! it can break is quiet: a missing `published_at` hides the app from the
//! launcher while direct URLs keep working; a build pointer with no bytes behind
//! it 404s only when someone clicks; a second `oxy dev seed` that trips the
//! `(org_id, slug)` unique index fails a developer's first command. None of that
//! shows up in a unit test, so it's tested here against the real thing.
//!
//! Uses testcontainers (falling back to `OXY_DATABASE_URL` in CI) so it runs on
//! a laptop as well as in CI — an env-gated skip would mean a broken seed passes
//! locally and only fails after push.

use crate::common::{APP_SLUG, demo_workspace_id, examples_path, test_db};
use entity::apps;
use entity::prelude::{AppBuilds, Apps, Organizations, Workspaces};
use oxy_api_custom_apps::custom_apps_build_store as store;
use oxy_api_custom_apps::seed;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

async fn seeded_app(db: &DatabaseConnection, org_slug: &str) -> apps::Model {
    let org = Organizations::find()
        .filter(entity::organizations::Column::Slug.eq(org_slug))
        .one(db)
        .await
        .expect("query org")
        .unwrap_or_else(|| panic!("seed did not create the {org_slug} org"));
    // Exactly the launcher's own filter — org membership aside, this is what
    // decides whether the app shows up on the home grid.
    Apps::find()
        .filter(apps::Column::OrgId.eq(org.id))
        .filter(apps::Column::Slug.eq(APP_SLUG))
        .filter(apps::Column::PublishedAt.is_not_null())
        .one(db)
        .await
        .expect("query app")
        .unwrap_or_else(|| panic!("no published {APP_SLUG} app in {org_slug}"))
}

#[tokio::test]
async fn seed_deploys_the_example_app_to_the_demo_workspace() {
    let db = test_db().await;
    seed::seed_demo(Some(examples_path()))
        .await
        .expect("seed_demo");

    let app = seeded_app(&db, "local").await;
    assert_eq!(
        app.project_id,
        demo_workspace_id(),
        "the app must hang off the demo workspace, or the launcher won't list it"
    );
    assert!(
        crate::app_environments::serving(&db, app.id)
            .await
            .production
            .is_some(),
        "production's build is what names the live bytes; without it the app 404s"
    );
    assert!(
        Workspaces::find_by_id(demo_workspace_id())
            .one(&db)
            .await
            .expect("query workspace")
            .is_some()
    );
}

#[tokio::test]
async fn seeded_app_bundle_is_actually_readable_from_the_build_store() {
    let db = test_db().await;
    seed::seed_demo(Some(examples_path()))
        .await
        .expect("seed_demo");
    let app = seeded_app(&db, "local").await;

    let published = crate::app_environments::serving(&db, app.id)
        .await
        .production
        .expect("published build");
    let build = AppBuilds::find_by_id(published)
        .one(&db)
        .await
        .expect("query build")
        .expect("production's build is a real app_builds row");

    // The DB naming a build proves nothing — this is the half that actually
    // 404s when it's missing, and it's the half a row-only assertion misses.
    let index = store::get_object(app.id, &build.build_id, "index.html")
        .await
        .expect("read index.html from the build store")
        .expect("index.html is stored under the build prefix");
    let html = String::from_utf8(index.to_vec()).expect("index.html is UTF-8");
    assert!(
        html.contains("</head>"),
        "the served bundle needs a </head> for runtime identity injection"
    );

    assert_eq!(
        build.validation_status, "passed",
        "the promote gate refuses to make a build live otherwise"
    );
    assert!(
        build.manifest_json.is_some(),
        "the launcher reads card metadata from manifest_json"
    );
}

#[tokio::test]
async fn seeding_twice_updates_in_place() {
    let db = test_db().await;
    // Idempotency is the whole contract of this command — `apps` is unique on
    // (org_id, slug), so a non-deterministic id would make the second run fail
    // outright.
    seed::seed_demo(Some(examples_path()))
        .await
        .expect("first seed");
    let first = seeded_app(&db, "local").await;
    seed::seed_demo(Some(examples_path()))
        .await
        .expect("second seed must not conflict");
    let second = seeded_app(&db, "local").await;

    assert_eq!(
        first.id, second.id,
        "re-seeding should update, not duplicate"
    );
    assert_eq!(
        Apps::find()
            .filter(apps::Column::Slug.eq(APP_SLUG))
            .all(&db)
            .await
            .expect("count apps")
            .len(),
        // One per seeded org: the demo workspace's, and Acme's.
        2,
        "re-seeding should not add rows"
    );
    assert_eq!(
        AppBuilds::find()
            .filter(entity::app_builds::Column::AppId.eq(first.id))
            .all(&db)
            .await
            .expect("count builds")
            .len(),
        1,
        "an unchanged bundle hashes to the same build id, so no second build row"
    );
}

#[tokio::test]
async fn seed_deploys_to_acme_so_the_admin_and_partner_surfaces_have_an_app() {
    let db = test_db().await;
    seed::seed_demo(Some(examples_path()))
        .await
        .expect("seed_demo");

    let acme = seeded_app(&db, "acme").await;
    let local = seeded_app(&db, "local").await;
    assert_ne!(
        acme.id, local.id,
        "one bundle, two independent deployments — not one row moved between orgs"
    );
    assert_eq!(
        acme.slug, local.slug,
        "the same slug in two orgs is legal and intended; apps is unique on (org_id, slug)"
    );
}

#[tokio::test]
async fn clear_removes_the_app_rows_and_their_bytes() {
    let db = test_db().await;
    seed::seed_demo(Some(examples_path()))
        .await
        .expect("seed_demo");
    let app = seeded_app(&db, "local").await;
    let published = crate::app_environments::serving(&db, app.id)
        .await
        .production
        .expect("build");
    let build = AppBuilds::find_by_id(published)
        .one(&db)
        .await
        .expect("query build")
        .expect("build row");

    seed::clear_demo().await.expect("clear_demo");

    assert!(
        Apps::find_by_id(app.id)
            .one(&db)
            .await
            .expect("query app")
            .is_none(),
        "apps.project_id has no FK, so dropping the workspace does not cascade — \
         clear must delete the app row itself or it dangles"
    );
    assert!(
        store::get_object(app.id, &build.build_id, "index.html")
            .await
            .expect("read after clear")
            .is_none(),
        "clear must remove the stored bytes too, or they leak with nothing referencing them"
    );
}
