//! Two console asks of the staff secrets surface, through the real handlers:
//!
//! - **`?build=` on the list** reconciles an environment's stored keys against
//!   a named build's declarations — "what would production be missing if this
//!   build went live" — so the promote and Make live confirms can warn. Another
//!   app's build is not found.
//! - **`copy_from` on the set** copies another environment's value server-side,
//!   only downhill (production → staging or a sandbox, staging → a sandbox),
//!   gated like a reveal of the source, and audited as a copy.

use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use entity::workspace_members::WorkspaceRole;
use oxy_api_custom_apps::custom_apps_secrets::{
    ListQuery, SetSecretRequest, admin_list, admin_set, workspace_set,
};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use oxy_server_authz::role_guards::WorkspaceAdmin;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::json;
use uuid::Uuid;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{FunctionSpec, Tenant, promote_staging, seeded_tenant};
use crate::staging_function_homes::publish_env_build;
use crate::staging_functions::make_guest_staff;
use crate::staging_secrets_admin::{
    app_with_both_values, request, use_a_fixed_encryption_key, user,
};

fn noop() -> FunctionSpec {
    FunctionSpec {
        name: "noop",
        manifest: json!({ "route": true }),
        js: "export default async () => Response.json({});",
    }
}

/// `app_builds.id` of the app's build published as `build_id`.
async fn build_pk(t: &Tenant, app: Uuid, build_id: &str) -> Uuid {
    entity::app_builds::Entity::find()
        .filter(entity::app_builds::Column::AppId.eq(app))
        .filter(entity::app_builds::Column::BuildId.eq(build_id))
        .one(&t.db)
        .await
        .expect("read build")
        .expect("the build")
        .id
}

fn list(build: Option<Uuid>) -> Query<ListQuery> {
    Query(ListQuery {
        environment: None,
        build,
    })
}

fn copy(key: &str, environment: Option<&str>, from: &str) -> Json<SetSecretRequest> {
    Json(
        serde_json::from_value(
            json!({ "key": key, "environment": environment, "copy_from": from }),
        )
        .expect("request"),
    )
}

#[tokio::test]
async fn a_named_build_is_reconciled_against_productions_keys() {
    use_a_fixed_encryption_key();
    let t = seeded_tenant().await;
    let live = json!({ "OLD": { "required": true } });
    let next = json!({ "OLD": { "required": true }, "NEW": { "required": true } });
    // Production refuses a build declaring a required key it has no value
    // for, so OLD is set before pw-1 goes live: publish, set, promote.
    let app = publish_env_build(&t, "promote-warn", &[noop()], live, "pw-1", false)
        .await
        .expect("pw-1 on staging")
        .app_id;
    make_guest_staff();
    let staff = || user(t.guest_id, LOCAL_GUEST_EMAIL);
    admin_set(Path(app), staff(), None, request("OLD", "v", None))
        .await
        .expect("production has OLD");
    promote_staging(&t, app).await;
    publish_env_build(&t, "promote-warn", &[noop()], next, "pw-2", false)
        .await
        .expect("staging's draft");

    let view = |build| async move {
        admin_list(
            Path(app),
            list(build),
            AuthenticatedUserExtractor(staff().user),
            None,
        )
        .await
    };
    let Json(now) = view(None).await.expect("production as it serves");
    assert_eq!(now.missing_required, 0, "the live build has all it needs");

    let draft = build_pk(&t, app, "pw-2").await;
    let Json(after) = view(Some(draft))
        .await
        .expect("production against the draft");
    assert_eq!(after.environment, "production");
    assert_eq!(after.declaring_build_id.as_deref(), Some("pw-2"));
    let missing: Vec<&str> = after
        .entries
        .iter()
        .filter(|e| e.is_missing_required())
        .map(|e| e.key.as_str())
        .collect();
    assert_eq!(missing, vec!["NEW"]);
    assert_eq!(after.missing_required, 1);

    // Another app's build names nothing here.
    let other = publish_env_build(&t, "promote-other", &[noop()], json!({}), "po-1", true)
        .await
        .expect("another app")
        .app_id;
    let theirs = build_pk(&t, other, "po-1").await;
    let (code, _) = view(Some(theirs)).await.expect_err("another app's build");
    assert_eq!(code, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn staff_copy_a_value_downhill_without_it_reaching_the_browser() {
    use entity::audit_events;
    let t = seeded_tenant().await;
    let app = app_with_both_values(&t, "copy-down").await;
    let staff = || user(t.guest_id, LOCAL_GUEST_EMAIL);
    admin_set(Path(app), staff(), None, request("ONLY_PROD", "p1", None))
        .await
        .expect("production's value");

    let status = admin_set(
        Path(app),
        staff(),
        None,
        copy("ONLY_PROD", Some("staging"), "production"),
    )
    .await
    .expect("copy production into staging");
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        stored(format!("apps/{app}/staging/ONLY_PROD"))
            .await
            .as_deref(),
        Some("p1")
    );

    // Uphill is refused, and production keeps its own value.
    let (code, _) = admin_set(Path(app), staff(), None, copy("QB_TOKEN", None, "staging"))
        .await
        .expect_err("staging into production");
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert_eq!(
        stored(format!("apps/{app}/QB_TOKEN")).await.as_deref(),
        Some("prod")
    );
    // A key the source does not hold.
    let (code, _) = admin_set(
        Path(app),
        staff(),
        None,
        copy("NOWHERE", Some("staging"), "production"),
    )
    .await
    .expect_err("nothing to copy");
    assert_eq!(code, StatusCode::NOT_FOUND);
    // Someone oxy-authz does not let open staging cannot copy into it.
    let tenant = || user(Uuid::new_v4(), "tenant-admin@customer.example");
    let (code, _) = admin_set(
        Path(app),
        tenant(),
        None,
        copy("ONLY_PROD", Some("staging"), "production"),
    )
    .await
    .expect_err("not staff");
    assert_eq!(code, StatusCode::FORBIDDEN);
    // The tenant mount reaches production alone, and nothing copies into it.
    let (code, _) = workspace_set(
        WorkspaceAdmin(WorkspaceRole::Admin),
        Path((demo_workspace_id(), app)),
        tenant(),
        None,
        copy("ONLY_PROD", None, "production"),
    )
    .await
    .expect_err("into production");
    assert_eq!(code, StatusCode::BAD_REQUEST);

    let copied: Vec<(String, String)> = audit_events::Entity::find()
        .filter(audit_events::Column::OrgId.eq(t.org_id))
        .filter(audit_events::Column::Action.eq("custom_app.secret.copied"))
        .filter(audit_events::Column::TargetId.contains(app.to_string()))
        .all(&t.db)
        .await
        .expect("audit rows")
        .into_iter()
        .map(|r| (r.environment, r.target_id.unwrap_or_default()))
        .collect();
    assert_eq!(
        copied,
        vec![(
            "staging".to_string(),
            format!("apps/{app}/staging/ONLY_PROD")
        )]
    );
}

async fn stored(name: String) -> Option<String> {
    oxy::service::secret_manager::SecretManagerService::new(demo_workspace_id())
        .get_secret(&name)
        .await
}
