//! App access (`/api/orgs/{org_id}/oxygen-access`): an org admin approving
//! what a custom app's live build asks to do in Oxygen itself, through the
//! served stack and the real authorization loader.
//!
//! What it must hold: the ask is read from the live build and nowhere else;
//! approving is a person in a browser session; the approval makes the app a
//! service account whose only reach is the approved actions, so the rest of
//! the org-admin ring stays shut; nobody can mint a token for that account;
//! the approval is a row of kind `app` in the org's inventory; and a revoke
//! ends it for the next call.

use axum::http::StatusCode;
use entity::org_members::OrgRole;
use entity::{app_builds, apps};
use oxy_app::server::authz::Caller;
use oxy_auth::types::AuthenticatedUser;
use oxy_authz::{Action, Resource};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement,
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::service_accounts::{admin_fixture, in_session, session_of, with_token};
use super::stack::{join_org, published_app};
use super::{Fixture, audit_rows, minted_pat, seed_user};

/// Give `app` a live build asking for `asks` in its `tablets` function.
async fn live_build(db: &DatabaseConnection, app: &apps::Model, asks: Value) {
    let id = Uuid::new_v4();
    app_builds::ActiveModel {
        id: ActiveValue::Set(id),
        app_id: ActiveValue::Set(app.id),
        build_id: ActiveValue::Set(format!("b-{}", id.simple())),
        s3_prefix: ActiveValue::Set(format!("customer-apps/{}/builds/b/", app.id)),
        manifest_json: ActiveValue::Set(Some(json!({ "functions": { "tablets": asks } }))),
        created_at: ActiveValue::Set(chrono::Utc::now().into()),
        validation_status: ActiveValue::Set("passed".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed build");
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO app_environments (app_id, name, kind, build_id) VALUES ($1, 'production', 'production', $2) \
         ON CONFLICT (app_id, name) DO UPDATE SET build_id = EXCLUDED.build_id",
        [app.id.into(), id.into()],
    ))
    .await
    .expect("point production");
}

async fn store_ops(fx: &Fixture) -> apps::Model {
    let app = published_app(&fx.db, fx.org_id, fx.workspace_id).await;
    live_build(
        &fx.db,
        &app,
        json!({ "route": true, "oxygen": { "frontline": { "devices": "manage" } } }),
    )
    .await;
    app
}

fn base(org: Uuid) -> String {
    format!("/orgs/{org}/oxygen-access")
}

/// Decide `action` as the app's own credential, through the real loader.
async fn app_decides(fx: &Fixture, app: &apps::Model, action: Action, resource: Resource) -> bool {
    let Some(credential) = oxy_auth::token::app_access::credential_for(&fx.db, app)
        .await
        .expect("read the app's access")
    else {
        return false;
    };
    let user = AuthenticatedUser {
        id: credential.principal_user_id,
        email: None,
        name: credential.name.clone(),
        picture: None,
        status: entity::users::UserStatus::Active,
        credential: Some(credential.clone()),
    };
    let caller = Caller::of(&user, Some(&credential));
    oxy_app::server::authz::enforce_for(
        &fx.db,
        &caller,
        "test.oxygen_access",
        action,
        resource,
        true,
    )
    .await
}

#[tokio::test]
async fn an_ask_shows_as_waiting_and_an_admin_approves_it_in_a_session() {
    let fx = admin_fixture().await;
    let app = store_ops(&fx).await;

    let (status, list) = in_session(&fx.cookie, "GET", &base(fx.org_id), None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let row = &list["apps"][0];
    assert_eq!(row["app_id"], app.id.to_string());
    assert_eq!(row["actions"][0]["action"], "frontline_device_manage");
    assert_eq!(row["actions"][0]["status"], "pending");
    assert_eq!(row["actions"][0]["functions"], json!(["tablets"]));
    assert!(row["token_id"].is_null());

    let (status, approved) = in_session(
        &fx.cookie,
        "POST",
        &format!("{}/{}/approve", base(fx.org_id), app.id),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    assert_eq!(approved["actions"][0]["status"], "approved");
    assert_eq!(approved["service_account"], format!("app-{}", app.slug));
    let token_id = approved["token_id"]
        .as_str()
        .expect("a row in the inventory")
        .to_string();

    // The approval is one row of kind `app` in the org's inventory.
    let (status, inventory) = in_session(
        &fx.cookie,
        "GET",
        &format!("/orgs/{}/tokens", fx.org_id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{inventory}");
    let listed = inventory["tokens"]
        .as_array()
        .expect("tokens")
        .iter()
        .find(|t| t["id"] == token_id.as_str())
        .unwrap_or_else(|| panic!("the app's row is missing from {inventory}"));
    assert_eq!(listed["kind"], "app");

    // It wrote the account's creation and the row's, under the approver.
    assert_eq!(audit_rows(&fx.db, "service_account.created").await.len(), 1);
    let created = audit_rows(&fx.db, "token.created").await;
    assert!(
        created
            .iter()
            .any(|r| r.metadata["approved_actions"] == json!(["frontline_device_manage"]))
    );
}

#[tokio::test]
async fn the_apps_credential_reaches_kiosks_and_nothing_else_of_the_ring() {
    let fx = admin_fixture().await;
    let app = store_ops(&fx).await;
    let org = Resource::org(fx.org_id);
    assert!(!app_decides(&fx, &app, Action::FrontlineDeviceManage, org.clone()).await);

    in_session(
        &fx.cookie,
        "POST",
        &format!("{}/{}/approve", base(fx.org_id), app.id),
        None,
    )
    .await;
    assert!(app_decides(&fx, &app, Action::FrontlineDeviceManage, org.clone()).await);
    for action in [
        Action::MemberSetRole,
        Action::MemberInvite,
        Action::MemberRemove,
        Action::ServiceAccountManage,
        Action::TokenInventoryView,
        Action::OrgBilling,
    ] {
        assert!(
            !app_decides(&fx, &app, action, org.clone()).await,
            "{action:?} must stay shut to an app approved for kiosks"
        );
    }
    let elsewhere = Resource::org(Uuid::new_v4());
    assert!(!app_decides(&fx, &app, Action::FrontlineDeviceManage, elsewhere).await);
}

#[tokio::test]
async fn approving_takes_a_person_in_a_session_who_is_an_org_admin() {
    let fx = admin_fixture().await;
    let app = store_ops(&fx).await;
    let approve = format!("{}/{}/approve", base(fx.org_id), app.id);

    // A token, even its owner's own, never approves.
    let (_, secret) = minted_pat(&fx, None).await;
    let (status, body) = with_token(&secret, "POST", &approve, Some(json!({}))).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "session_required");

    // A plain member is refused.
    let member = seed_user(&fx.db, "member").await;
    join_org(&fx.db, fx.org_id, member.id, OrgRole::Member).await;
    let (status, _) = in_session(&session_of(&member).await, "POST", &approve, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // An action the live build does not ask for cannot be approved.
    let (status, body) = in_session(
        &fx.cookie,
        "POST",
        &approve,
        Some(json!({ "actions": ["source_upload_report"] })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "not_declared");
    assert!(
        oxy_auth::token::app_access::credential_for(&fx.db, &app)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn no_token_or_trust_policy_is_ever_made_for_the_apps_account() {
    let fx = admin_fixture().await;
    let app = store_ops(&fx).await;
    in_session(
        &fx.cookie,
        "POST",
        &format!("{}/{}/approve", base(fx.org_id), app.id),
        None,
    )
    .await;
    let account = oxy_auth::token::app_access::account_of(&fx.db, app.id)
        .await
        .unwrap()
        .expect("the approval made an account");
    let (status, body) = in_session(
        &fx.cookie,
        "POST",
        &format!(
            "/orgs/{}/service-accounts/{}/tokens",
            fx.org_id, account.user_id
        ),
        Some(json!({ "name": "sneaky", "expires_in_days": 30 })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "app_account");

    // Nor through a trust policy: its CI tokens would carry the account's
    // admin standing without the approval's narrowing.
    let (status, body) = in_session(
        &fx.cookie,
        "POST",
        &super::oidc::policies_uri(fx.org_id, account.user_id),
        Some(super::oidc::policy_body(json!([super::oidc::whole_org(
            "admin"
        )]))),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "app_account");

    // Nor through an OIDC issuer trust policy, for the same reason: its
    // tokens would carry the same standing.
    super::oidc_issuer_access::trust_issuers();
    let issuer = super::oidc_issuer_access::the_issuer(&fx).await;
    let (status, body) = in_session(
        &fx.cookie,
        "POST",
        &super::oidc_issuer_access::policies_uri(fx.org_id, account.user_id),
        Some(super::oidc_issuer_access::policy_body(
            issuer,
            json!([super::oidc::whole_org("admin")]),
        )),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "app_account");
}

#[tokio::test]
async fn a_revoke_ends_the_apps_access_for_the_next_call() {
    let fx = admin_fixture().await;
    let app = store_ops(&fx).await;
    in_session(
        &fx.cookie,
        "POST",
        &format!("{}/{}/approve", base(fx.org_id), app.id),
        None,
    )
    .await;
    assert!(
        app_decides(
            &fx,
            &app,
            Action::FrontlineDeviceManage,
            Resource::org(fx.org_id)
        )
        .await
    );

    let (status, after) = in_session(
        &fx.cookie,
        "POST",
        &format!("{}/{}/revoke", base(fx.org_id), app.id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{after}");
    assert!(after["token_id"].is_null());
    assert_eq!(after["actions"][0]["status"], "pending");
    assert!(
        !app_decides(
            &fx,
            &app,
            Action::FrontlineDeviceManage,
            Resource::org(fx.org_id)
        )
        .await
    );
    assert!(
        audit_rows(&fx.db, "token.revoked")
            .await
            .iter()
            .any(|r| r.metadata["reason"] == "app access revoked")
    );

    // Approving again makes a new row for the same account.
    let (status, again) = in_session(
        &fx.cookie,
        "POST",
        &format!("{}/{}/approve", base(fx.org_id), app.id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["service_account"], format!("app-{}", app.slug));
}

/// An upload asks on named pipelines, and is approved and shown per pipeline:
/// a later build naming one more shows only that one waiting, and the ones
/// approved keep working.
#[tokio::test]
async fn an_upload_is_approved_and_shown_per_pipeline() {
    let fx = admin_fixture().await;
    let app = published_app(&fx.db, fx.org_id, fx.workspace_id).await;
    let uploads =
        |pipelines: &[&str]| json!({ "oxygen": { "sourceUploads": { "pipelines": pipelines } } });
    live_build(
        &fx.db,
        &app,
        uploads(&["ubereats_transactions", "toast_pos"]),
    )
    .await;
    let one = format!("{}/{}", base(fx.org_id), app.id);

    let (status, before) = in_session(&fx.cookie, "GET", &one, None).await;
    assert_eq!(status, StatusCode::OK, "{before}");
    let action = &before["actions"][0];
    assert_eq!(action["action"], "source_upload_report");
    assert_eq!(action["status"], "pending");
    assert_eq!(
        action["targets"]
            .as_array()
            .expect("targets")
            .iter()
            .map(|t| (t["name"].as_str().unwrap(), t["status"].as_str().unwrap()))
            .collect::<Vec<_>>(),
        [
            ("toast_pos", "pending"),
            ("ubereats_transactions", "pending")
        ]
    );

    let (status, approved) = in_session(
        &fx.cookie,
        "POST",
        &format!("{one}/approve"),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    assert_eq!(approved["actions"][0]["status"], "approved");
    let grants = oxy_auth::token::app_access::approvals(&fx.db, app.id)
        .await
        .expect("approvals");
    let mut targets: Vec<_> = grants.iter().filter_map(|g| g.target.clone()).collect();
    targets.sort();
    assert_eq!(
        targets,
        ["toast_pos", "ubereats_transactions"],
        "one row per pipeline, none org-wide"
    );
    let created = audit_rows(&fx.db, "token.created").await;
    assert!(
        created
            .iter()
            .any(|r| r.metadata["approved_pipelines"]
                == json!(["toast_pos", "ubereats_transactions"]))
    );

    // A new build asks for one more pipeline: only it waits.
    live_build(
        &fx.db,
        &app,
        uploads(&["ubereats_transactions", "toast_pos", "weather"]),
    )
    .await;
    let (_, after) = in_session(&fx.cookie, "GET", &one, None).await;
    let action = &after["actions"][0];
    assert_eq!(action["status"], "pending");
    let status_of = |name: &str| {
        action["targets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .map(|t| t["status"].clone())
    };
    assert_eq!(status_of("weather"), Some(json!("pending")));
    assert_eq!(status_of("toast_pos"), Some(json!("approved")));
}

/// Seed a promoted revision of `workspace` holding the pipelines `names`.
async fn promoted_with(db: &DatabaseConnection, workspace: Uuid, names: &[&str]) {
    let revision = Uuid::new_v4();
    let now = chrono::Utc::now().fixed_offset();
    entity::revisions::ActiveModel {
        revision_id: ActiveValue::Set(revision),
        workspace_id: ActiveValue::Set(workspace),
        git_sha: ActiveValue::Set(format!("sha-{}", revision.simple())),
        branch: ActiveValue::Set(Some("main".into())),
        schema_version: ActiveValue::Set(1),
        status: ActiveValue::Set("ready".into()),
        kind: ActiveValue::Set("main".into()),
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
    for name in names {
        entity::airway_pipelines::ActiveModel {
            revision_id: ActiveValue::Set(revision),
            name: ActiveValue::Set((*name).into()),
            file_path: ActiveValue::Set(format!("pipelines/{name}.airway.yml")),
            definition: ActiveValue::Set(json!({ "name": name })),
        }
        .insert(db)
        .await
        .expect("seed pipeline");
    }
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE workspaces SET current_revision_id = $1 WHERE id = $2",
        [revision.into(), workspace.into()],
    ))
    .await
    .expect("promote");
}

/// `ctx.oxygen.sourceUploads.report` names a pipeline; Oxygen finds the one
/// workspace of the org whose promoted revision holds it, and refuses a name
/// that is in none or in more than one. Another org's pipelines never count.
#[tokio::test]
async fn an_upload_pipeline_is_found_by_name_in_one_place_in_the_org() {
    use oxy_api_airway::app_ops::resolve;
    use oxy_app_core::app_platform_seam::PlatformActionError;

    let fx = admin_fixture().await;
    promoted_with(
        &fx.db,
        fx.workspace_id,
        &["ubereats_transactions", "shared"],
    )
    .await;
    let second = super::seed_workspace_in(&fx.db, fx.org_id).await;
    promoted_with(&fx.db, second, &["shared"]).await;
    let (_, theirs) = super::seed_workspace(&fx.db).await;
    promoted_with(&fx.db, theirs, &["theirs"]).await;

    let found = resolve(&fx.db, fx.org_id, "ubereats_transactions")
        .await
        .expect("found");
    assert_eq!(found.workspace_id, fx.workspace_id);
    assert_eq!(
        found.file_path,
        "pipelines/ubereats_transactions.airway.yml"
    );

    assert!(matches!(
        resolve(&fx.db, fx.org_id, "shared").await,
        Err(PlatformActionError::Invalid(m)) if m.contains("more than one")
    ));
    for missing in ["nope", "theirs"] {
        assert!(
            matches!(
                resolve(&fx.db, fx.org_id, missing).await,
                Err(PlatformActionError::NotFound(_))
            ),
            "{missing}"
        );
    }
}
