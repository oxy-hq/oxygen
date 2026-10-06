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

/// `app_publish` on a personal token: what the combination does today.
mod personal_grant;

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
        publisher: who
            .published_by
            .zip(who.published_by_email.as_deref())
            .map(|(id, email)| oxy_app::server::authz::Caller::without_credential(id, email)),
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

// ── Trusted access: a trust policy's `app_publish` grant (API-tokens §3.4) ───
//
// The newer way to the same publish. A GitHub Actions run exchanges its OIDC
// token for an `oxy_ci_` token that acts as a service account of the app's own
// org; the grant on that token is what authorizes the publish. The legacy
// path above is unchanged, and still asks for the client's consent.

/// A real `oxy_ci_` credential: an account of `t`'s org, a trust policy
/// granting `grants`, a token minted from it, and that token admitted through
/// the same authenticator every request goes through.
async fn ci_publisher(
    db: &DatabaseConnection,
    t: &Tenant,
    grants: Vec<oxy_auth::token::trust_policy_access::PolicyGrant>,
) -> (Publisher, Uuid) {
    use oxy_auth::authenticator::Authenticator;
    use oxy_auth::token::account_access::{AccountRole, AccountWant};
    use oxy_auth::token::trust_policy_access::RepoIds;
    use oxy_auth::token::{ci, service_account, trust_policy};

    oxy_auth::built_in::set_auth_configured(true);
    oxy_auth::token::cache::clear();
    let want = AccountWant {
        name: "publisher".into(),
        description: None,
        role: AccountRole::Member,
    };
    let account = service_account::create(db, t.org_id, want, t.admin.id)
        .await
        .expect("create the service account");
    let policy = trust_policy::create(
        db,
        trust_policy::NewPolicy {
            org_id: t.org_id,
            service_account_id: account.user_id,
            repository: "acme/app".into(),
            ids: RepoIds {
                repository_id: 987,
                repository_owner_id: 42,
            },
            workflow_path: ".github/workflows/oxy-publish.yml".into(),
            environment: Some("production".into()),
            ref_pattern: None,
            allow_self_hosted: false,
            grants: grants.clone(),
            created_by: t.admin.id,
        },
    )
    .await
    .expect("register the trust policy");
    let minted = ci::mint(
        db,
        ci::NewCiToken {
            account_id: account.user_id,
            policy_id: policy.id,
            name: IDENTITY.to_string(),
            grants,
            claims: serde_json::json!({ "run_id": "7001" }),
            now: chrono::Utc::now(),
        },
    )
    .await
    .expect("mint the ci token");

    let mut headers = axum::http::HeaderMap::new();
    let bearer = format!("Bearer {}", minted.secret);
    headers.insert("authorization", bearer.parse().unwrap());
    let (_identity, credential) =
        oxy_auth::built_in::BuiltInAuthenticator::new(oxy_auth::token::SandboxAgent::Refuse)
            .authenticate_with_credential(&headers)
            .await
            .expect("the ci token authenticates");
    let row = users::Entity::find_by_id(account.user_id)
        .one(db)
        .await
        .expect("query users")
        .expect("the account's users row");
    let user = AuthenticatedUser::from(row).with_credential(credential);
    (Publisher::from_request(&user, None), account.user_id)
}

/// `input`, carrying the publisher's own credential — as `publish_handler`
/// hands it on.
fn input_as(t: &Tenant, slug: &str, build_id: &str, who: Publisher) -> PublishInput {
    let caller = who.caller.clone();
    PublishInput {
        publisher: caller,
        ..input(t, slug, build_id, false, who)
    }
}

async fn revoke_consent(db: &DatabaseConnection, org_id: Uuid) {
    partner_publish_consent::Entity::delete_by_id(org_id)
        .exec(db)
        .await
        .expect("revoke the client's consent");
}

#[tokio::test]
async fn a_trust_policy_grant_publishes_its_app_and_no_other_without_consent() {
    use oxy_auth::token::trust_policy_access::PolicyGrant;

    let db = test_db().await;
    let t = seed_tenant(&db).await;
    let granted = human_first_publish(&db, &t, "granted-app").await;
    human_first_publish(&db, &t, "other-app").await;
    // The org never agreed to anyone else publishing into it.
    revoke_consent(&db, t.org_id).await;

    let grant = PolicyGrant::AppPublish {
        org_id: t.org_id,
        app_id: granted,
    };
    let (who, account) = ci_publisher(&db, &t, vec![grant]).await;
    assert_eq!(
        who.published_by,
        Some(account),
        "the account is the publisher"
    );
    assert_eq!(who.published_via.as_deref(), Some(IDENTITY));

    // Its own app: published, with no consent row — the org's own policy is
    // the org publishing its own app.
    let result = publish(input_as(&t, "granted-app", "ci-grant-1", who.clone()))
        .await
        .expect("a trust policy's grant publishes its app without consent");
    assert_eq!(result.app_id, granted);
    let build = build_row(&db, granted, "ci-grant-1").await;
    assert_eq!(
        build.published_by,
        Some(account),
        "the account has a users row, so the build names it"
    );
    assert_eq!(build.published_via.as_deref(), Some(IDENTITY));

    // Another app of the same org: refused. The grant names one app.
    let err = publish(input_as(&t, "other-app", "ci-grant-2", who.clone()))
        .await
        .expect_err("the grant does not reach another app");
    assert!(
        matches!(err, PublishError::OxyAccessDenied { .. }),
        "{err:?}"
    );
    // And it cannot bring a new app into being.
    let err = publish(input_as(&t, "brand-new-app", "ci-grant-3", who))
        .await
        .expect_err("a grant publishes an app that exists");
    assert!(
        matches!(err, PublishError::OxyAccessDenied { .. }),
        "{err:?}"
    );

    // The legacy machine path still asks for consent, and is refused without it.
    let machine = Publisher::from_request(
        &AuthenticatedUser::machine_publisher(),
        Some(&machine_marker(granted)),
    );
    let err = publish(input(&t, "granted-app", "ci-legacy-1", false, machine))
        .await
        .expect_err("the legacy exchange's token still needs the client's consent");
    assert!(
        matches!(err, PublishError::OxyAccessDenied { .. }),
        "{err:?}"
    );
}

#[tokio::test]
async fn a_ci_token_without_an_app_publish_grant_cannot_publish() {
    use oxy_auth::token::personal::GrantSpec;
    use oxy_auth::token::trust_policy_access::PolicyGrant;

    let db = test_db().await;
    let t = seed_tenant(&db).await;
    human_first_publish(&db, &t, "an-app").await;
    // The whole org, as a member — and no word about publishing.
    let grant = PolicyGrant::Workspace(GrantSpec {
        org_id: t.org_id,
        workspace_id: None,
        ceiling: oxy_authz::RoleCeiling::Member,
    });
    let (who, _account) = ci_publisher(&db, &t, vec![grant]).await;
    let err = publish(input_as(&t, "an-app", "ci-nogrant-1", who))
        .await
        .expect_err("reaching the org is not a grant to publish into it");
    assert!(
        matches!(err, PublishError::OxyAccessDenied { .. }),
        "{err:?}"
    );
}
