//! A new org arrives with no workspace.
//!
//! Customers no longer create orgs: Oxy staff (`POST /admin/orgs`) and partners
//! onboard them. A workspace exists only once a repository backs it
//! (`internal-docs/factory-retirement.md` phase 6), so creation leaves the org
//! alone and it shows its setup screen until the repository is connected. That
//! also makes creation Postgres-only: it works on a serve replica, where the old
//! blank `Default` workspace could not be scaffolded.
//!
//! Drives the real handler against a per-test database (`common::test_db`).
//! Global Owner standing comes from `OXY_OWNER`, the one standing that needs no
//! grant row; nextest gives each test its own process, so setting it here
//! reaches no other test.

use axum::Json;
use axum::http::HeaderMap;
use entity::{organizations, users, workspaces};
use oxy_api_tenancy::admin::orgs::{AdminCreateOrgBody, create_org};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};
use uuid::Uuid;

use crate::common::test_db;

/// A live user who is also the Global Owner, so `PlatformOrgCreate` allows them.
async fn seed_owner(db: &DatabaseConnection) -> users::Model {
    let id = Uuid::new_v4();
    let email = format!("staff-{id}@example.com");
    let user = users::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(Some(email.clone())),
        name: ActiveValue::Set("Staff".into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        status: ActiveValue::Set(users::UserStatus::Active),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed staff user");
    // SAFETY: process-per-test (asserted by `test_db`), before any request runs.
    unsafe { std::env::set_var("OXY_OWNER", &email) };
    user
}

fn actor(user: &users::Model) -> AuthenticatedUserExtractor {
    AuthenticatedUserExtractor(AuthenticatedUser {
        id: user.id,
        email: user.email.clone(),
        name: user.name.clone(),
        picture: None,
        status: users::UserStatus::Active,
        credential: None,
    })
}

/// The owner is the staff member themselves: an existing user is seeded as a
/// member, so no invitation email is sent.
fn body(user: &users::Model, slug: &str) -> Json<AdminCreateOrgBody> {
    Json(AdminCreateOrgBody {
        name: "Default Workspace Co".into(),
        slug: Some(slug.to_string()),
        owner_email: user.email.clone().expect("seeded with an email"),
    })
}

fn fresh_slug() -> String {
    format!("dw-{}", Uuid::new_v4().simple())
}

#[tokio::test]
async fn admin_create_org_creates_no_workspace() {
    let db = test_db().await;
    let staff = seed_owner(&db).await;

    let Json(created) = create_org(actor(&staff), HeaderMap::new(), body(&staff, &fresh_slug()))
        .await
        .expect("staff create org");

    let rows = workspaces::Entity::find()
        .filter(workspaces::Column::OrgId.eq(created.org.id))
        .all(&db)
        .await
        .expect("load workspaces");
    assert!(rows.is_empty(), "a new org has no workspace: {rows:?}");
    assert_eq!(created.org.workspace_count, 0);
}

/// Creation writes only Postgres, so a serve replica (no working copy) answers
/// it: the org is created, not refused.
#[tokio::test]
async fn a_serve_replica_creates_an_org() {
    let db = test_db().await;
    let staff = seed_owner(&db).await;
    // SAFETY: process-per-test; the role is read once, into a once-cell.
    unsafe { std::env::set_var("OXY_ROLE", "serve") };
    oxy_app::surface::roles::init_process_role_from_env();

    let slug = fresh_slug();
    let Json(created) = create_org(actor(&staff), HeaderMap::new(), body(&staff, &slug))
        .await
        .expect("a serve replica creates the org");
    let org = organizations::Entity::find_by_id(created.org.id)
        .one(&db)
        .await
        .expect("look up org");
    assert_eq!(org.map(|o| o.slug), Some(slug));
}

// `the_workspace_list_never_offers_the_local_mode_nil_workspace` moved to
// `oxy-api-workspaces`' integration group (`workspace_list`) with
// `list_workspaces` (2026-11-16): S3 forbids this crate depending on that
// one, dev dependencies included. It seeds the org there by rows rather than
// through `create_org`.
