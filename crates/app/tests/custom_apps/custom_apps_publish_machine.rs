//! A trusted-publishing (GitHub OIDC) publish records a build.
//!
//! The machine token authenticates as `AuthenticatedUser::machine_publisher()`,
//! whose nil id has no `users` row. `publish_handler` used to stamp that id as
//! `published_by`, and `app_builds.published_by` (plus the environment
//! `updated_by` / `actor` columns `set_pointers` writes) reference `users(id)` —
//! so every machine publish 500'd on `fk_app_builds_published_by` and no
//! trusted publish ever succeeded.
//!
//! These drive the real `publish()` against a per-test database, with the
//! publisher fields built by the same `Publisher::from_request` the handler
//! uses, so the FK is exercised exactly as production hits it.

use crate::common::test_db;
use entity::{
    app_builds, app_environment_events, app_environments, apps, org_members, org_members::OrgRole,
    organizations, partner_publish_consent, users, workspaces,
};
use flate2::{Compression, write::GzEncoder};
use oxy_app::server::api::custom_apps_publish::{
    OrgRef, PublishError, PublishInput, Publisher, publish,
};
use oxy_auth::types::{AppPublishTokenAuth, AuthenticatedUser};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};
use uuid::Uuid;

const IDENTITY: &str =
    "github-oidc:acme/app/.github/workflows/oxy-publish.yml@refs/heads/main env=production";

struct Tenant {
    org_id: Uuid,
    admin: AuthenticatedUser,
    workspace: Uuid,
}

async fn seed_tenant(db: &DatabaseConnection) -> Tenant {
    let org_id = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org_id),
        name: ActiveValue::Set("Machine Publish Org".into()),
        slug: ActiveValue::Set(format!("mach-{}", &org_id.simple().to_string()[..12])),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed org");

    let user_id = Uuid::new_v4();
    let admin = users::ActiveModel {
        id: ActiveValue::Set(user_id),
        email: ActiveValue::Set(Some(format!("admin-{user_id}@example.com"))),
        name: ActiveValue::Set("Admin".into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed user");

    org_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(org_id),
        user_id: ActiveValue::Set(user_id),
        role: ActiveValue::Set(OrgRole::Admin),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed org member");

    let workspace = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(workspace),
        name: ActiveValue::Set("Workspace".into()),
        org_id: ActiveValue::Set(Some(org_id)),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed workspace");

    // The client's consent — the machine path requires it (a revoke denies).
    partner_publish_consent::ActiveModel {
        org_id: ActiveValue::Set(org_id),
        enabled: ActiveValue::Set(true),
        granted_by: ActiveValue::Set(Some(user_id)),
        updated_at: ActiveValue::Set(chrono::Utc::now().fixed_offset()),
    }
    .insert(db)
    .await
    .expect("seed consent");

    Tenant {
        org_id,
        admin: AuthenticatedUser::from(admin),
        workspace,
    }
}

/// The smallest bundle `validate_bundle` accepts: an index.html with a head.
fn bundle() -> Vec<u8> {
    let html = b"<!doctype html><html><head><title>t</title></head><body></body></html>";
    let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    let mut header = tar::Header::new_gnu();
    header.set_size(html.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(&mut header, "index.html", &html[..])
        .expect("append index.html");
    builder
        .into_inner()
        .expect("finish tar")
        .finish()
        .expect("finish gzip")
}

fn input(t: &Tenant, slug: &str, build_id: &str, promote: bool, who: Publisher) -> PublishInput {
    PublishInput {
        org_ref: Some(OrgRef::Id(t.org_id)),
        app_slug: slug.to_string(),
        project_id: t.workspace,
        branch: None,
        build_id: build_id.to_string(),
        name: None,
        promote,
        tarball: bundle(),
        manifest: None,
        source_repo: None,
        commit_sha: None,
        published_by: who.published_by,
        published_by_email: who.published_by_email,
        machine_app_id: who.machine_app_id,
        published_via: who.published_via,
        semantic_revision_id: None,
    }
}

/// The marker `auth_middleware` stamps for an OIDC-minted token of `app_id`.
fn machine_marker(app_id: Uuid) -> AppPublishTokenAuth {
    AppPublishTokenAuth {
        token_id: Uuid::new_v4(),
        app_id: Some(app_id),
        machine_identity: Some(IDENTITY.to_string()),
    }
}

async fn build_row(db: &DatabaseConnection, app_id: Uuid, build_id: &str) -> app_builds::Model {
    app_builds::Entity::find()
        .filter(app_builds::Column::AppId.eq(app_id))
        .filter(app_builds::Column::BuildId.eq(build_id))
        .one(db)
        .await
        .expect("query app_builds")
        .expect("build row inserted")
}

/// A human publish creates the app (trusted publishing never creates one).
async fn human_first_publish(db: &DatabaseConnection, t: &Tenant, slug: &str) -> Uuid {
    let human = Publisher::from_request(&t.admin, None);
    let first = publish(input(t, slug, "human-1", false, human))
        .await
        .expect("human publish");
    let row = build_row(db, first.app_id, "human-1").await;
    assert_eq!(
        row.published_by,
        Some(t.admin.id),
        "a human stamps their user"
    );
    assert_eq!(row.published_via, None);
    first.app_id
}

#[tokio::test]
async fn a_machine_publish_inserts_a_build_attributed_to_its_workflow() {
    let db = test_db().await;
    let t = seed_tenant(&db).await;
    let app_id = human_first_publish(&db, &t, "machine-app").await;

    // Promoting, so `set_pointers` also writes both environments and their
    // events — the other two `users` FKs the nil id used to hit.
    let machine = Publisher::from_request(
        &AuthenticatedUser::machine_publisher(),
        Some(&machine_marker(app_id)),
    );
    let result = publish(input(&t, "machine-app", "ci-1", true, machine))
        .await
        .expect("a trusted publish must record its build, not 500 on the users FK");
    assert_eq!(result.app_id, app_id);

    let build = build_row(&db, app_id, "ci-1").await;
    assert_eq!(build.published_by, None, "no user published this build");
    assert_eq!(build.published_via.as_deref(), Some(IDENTITY));

    let app = apps::Entity::find_by_id(app_id)
        .one(&db)
        .await
        .expect("reload app")
        .expect("app exists");
    assert_eq!(
        app.published_build_id,
        Some(build.id),
        "the build went live"
    );

    let envs = app_environments::Entity::find()
        .filter(app_environments::Column::AppId.eq(app_id))
        .filter(app_environments::Column::BuildId.eq(build.id))
        .all(&db)
        .await
        .expect("query app_environments");
    assert_eq!(envs.len(), 2, "staging and production both moved");
    assert!(envs.iter().all(|e| e.updated_by.is_none()));

    let events = app_environment_events::Entity::find()
        .filter(app_environment_events::Column::BuildId.eq(build.id))
        .all(&db)
        .await
        .expect("query app_environment_events");
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|e| e.actor.is_none()));
}

/// The regression the fix removes, pinned so the test above can't pass
/// vacuously: the principal's nil id is not a user, and the FK says so.
#[tokio::test]
async fn the_machine_principal_id_cannot_be_recorded_as_a_publisher() {
    let db = test_db().await;
    let t = seed_tenant(&db).await;
    let app_id = human_first_publish(&db, &t, "nil-app").await;

    let mut old_shape = Publisher::from_request(
        &AuthenticatedUser::machine_publisher(),
        Some(&machine_marker(app_id)),
    );
    old_shape.published_by = Some(AuthenticatedUser::machine_publisher().id);
    let err = publish(input(&t, "nil-app", "ci-nil", false, old_shape))
        .await
        .expect_err("the nil principal has no users row");
    match err {
        PublishError::Db(msg) => assert!(
            msg.contains("fk_app_builds_published_by"),
            "unexpected db error: {msg}"
        ),
        other => panic!("expected the published_by FK violation, got {other:?}"),
    }
}
