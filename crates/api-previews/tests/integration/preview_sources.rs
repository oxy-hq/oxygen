//! I4 — `PUT /previews/sources`: a QuickBooks sample's sandbox company can
//! never name production's grant. The promoted revision runs two QuickBooks
//! pipelines (one rotating, one in read-only custody); every var either names,
//! and the realm the first reads, is refused, and one sandbox var is rotated by
//! one pipeline only. Database-backed (`Schema::All`).

use axum::http::StatusCode;
use sea_orm::{ActiveModelTrait, ActiveValue, ConnectionTrait, DatabaseBackend, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::preview_routes::fixture::{Fx, pipeline, send_json, setup};

const PROD_REALM: i64 = 9341456860808037;

fn quickbooks(name: &str, config: Value) -> Value {
    json!({ "name": name, "source": { "kind": "quickbooks", "config": config },
            "destination": { "database": "airhouse", "dataset_name": name } })
}

async fn world() -> Fx {
    let fx = setup().await;
    let main: Uuid = fx
        .db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT current_revision_id FROM workspaces WHERE id = $1",
            [fx.ws.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "current_revision_id")
        .unwrap();
    let rotating = json!({ "client_id": "PROD", "client_secret_var": "QB_CLIENT_SECRET",
                           "refresh_token_var": "QB_REFRESH_TOKEN__EASTBAY", "realm_id": PROD_REALM });
    pipeline(
        &fx.db,
        main,
        "airway/qb_eastbay.airway.yml",
        quickbooks("quickbooks_financials_eastbay", rotating),
    )
    .await;
    let read_only = json!({ "client_id": "PROD", "access_token_var": "apps/app-1/QB_ACCESS_TOKEN",
                            "realm_id": "1234567890" });
    pipeline(
        &fx.db,
        main,
        "airway/qb_west.airway.yml",
        quickbooks("quickbooks_financials_west", read_only),
    )
    .await;
    fx
}

fn sandbox(pipeline: &str, overrides: Value) -> Value {
    json!({ "pipeline": pipeline, "environment": "sandbox", "overrides": overrides })
}

fn good() -> Value {
    json!({ "realm_id": "4620816365000000",
            "refresh_token_var": "QB_SANDBOX_REFRESH_TOKEN__EASTBAY",
            "client_id_var": "QB_SANDBOX_CLIENT_ID",
            "client_secret_var": "QB_SANDBOX_CLIENT_SECRET" })
}

async fn put(fx: &Fx, body: Value) -> (StatusCode, Value) {
    send_json(
        &fx.staff,
        "PUT",
        format!("/{}/previews/sources", fx.ws),
        Some(body),
    )
    .await
}

#[tokio::test]
async fn save_refuses_a_production_var_or_realm() {
    let fx = world().await;
    let eastbay = "quickbooks_financials_eastbay";
    let with = |key: &str, value: Value| {
        let mut o = good();
        o[key] = value;
        o
    };
    for (overrides, code) in [
        (
            with("refresh_token_var", json!("QB_REFRESH_TOKEN__EASTBAY")),
            "production_var",
        ),
        (
            with("client_secret_var", json!("QB_CLIENT_SECRET")),
            "production_var",
        ),
        (
            json!({ "realm_id": "4620816365000000",
                    "access_token_var": "apps/app-1/QB_ACCESS_TOKEN" }),
            // App-scoped: refused as reserved before production is consulted.
            "reserved_var",
        ),
        (
            with("realm_id", json!(PROD_REALM.to_string())),
            "production_realm",
        ),
        (with("realm_id", json!("1234567890")), "production_realm"),
    ] {
        let (status, body) = put(&fx, sandbox(eastbay, overrides.clone())).await;
        assert_eq!(status, StatusCode::CONFLICT, "{overrides}: {body}");
        assert_eq!(body["code"], code, "{overrides}: {body}");
    }
    let (status, body) = send_json(
        &fx.staff,
        "GET",
        format!("/{}/previews/sources", fx.ws),
        None,
    )
    .await;
    assert_eq!(
        (status, body.clone()),
        (StatusCode::OK, json!([])),
        "nothing refused was stored"
    );

    // The control: the sandbox's own names and company are saved.
    let (status, saved) = put(&fx, sandbox(eastbay, good())).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["pipeline"], eastbay);
    assert_eq!(saved["environment"], "sandbox");
    assert_eq!(saved["overrides"], good());
    assert_eq!(saved["updated_by"], fx.staff.id.to_string());

    // One rotator per grant: another pipeline may not rotate the same var…
    let (status, body) = put(&fx, sandbox("quickbooks_financials_west", good())).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "rotating_var_taken");
    // …but the same pipeline may change its row.
    let moved = json!({ "realm_id": "4620816365000001",
                        "refresh_token_var": "QB_SANDBOX_REFRESH_TOKEN__EASTBAY",
                        "client_secret_var": "QB_SANDBOX_CLIENT_SECRET" });
    let (status, body) = put(&fx, sandbox(eastbay, moved.clone())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, listed) = send_json(
        &fx.staff,
        "GET",
        format!("/{}/previews/sources", fx.ws),
        None,
    )
    .await;
    assert_eq!(listed.as_array().map(Vec::len), Some(1), "{listed}");
    assert_eq!(listed[0]["overrides"], moved);
}

#[tokio::test]
async fn sources_are_staff_only_and_sandbox_only() {
    let fx = world().await;
    let (status, _) = send_json(
        &fx.customer,
        "PUT",
        format!("/{}/previews/sources", fx.ws),
        Some(sandbox("quickbooks_financials_eastbay", good())),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "an org admin is not staff");
    let (status, _) = send_json(
        &fx.customer,
        "GET",
        format!("/{}/previews/sources", fx.ws),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let mut production = sandbox("quickbooks_financials_eastbay", good());
    production["environment"] = json!("production");
    let (status, body) = put(&fx, production).await;
    assert_eq!(
        (status, body["code"].clone()),
        (StatusCode::BAD_REQUEST, json!("bad_request"))
    );
    let secret_value = sandbox(
        "quickbooks_financials_eastbay",
        json!({ "realm_id": "4620816365000000", "refresh_token": "a-real-token" }),
    );
    let (status, _) = put(&fx, secret_value).await;
    assert!(
        status.is_client_error(),
        "a secret value is never accepted: {status}"
    );
}

/// A custom app in the workspace: its manifest override declares an `env` key,
/// and a build's function manifest a webhook `secretVar` pair.
async fn seed_app(fx: &Fx) {
    let org: Uuid = fx
        .db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT org_id FROM workspaces WHERE id = $1",
            [fx.ws.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "org_id")
        .unwrap();
    let app_id = Uuid::new_v4();
    entity::apps::ActiveModel {
        id: ActiveValue::Set(app_id),
        org_id: ActiveValue::Set(org),
        project_id: ActiveValue::Set(fx.ws),
        slug: ActiveValue::Set(format!("qb-refresh-{}", app_id.simple())),
        name: ActiveValue::Set("QB refresh".into()),
        branch: ActiveValue::Set("main".into()),
        source_repo: ActiveValue::Set("git@example.com:acme/qb.git".into()),
        status: ActiveValue::Set("active".into()),
        source_type: ActiveValue::Set("git".into()),
        source_config: ActiveValue::Set(json!({})),
        visibility: ActiveValue::Set("org".into()),
        manifest_override: ActiveValue::Set(Some(
            json!({ "env": { "QB_REFRESH_TOKEN_EASTBAY": { "required": true } } }),
        )),
        ..Default::default()
    }
    .insert(&fx.db)
    .await
    .expect("seed app");
    entity::app_builds::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        app_id: ActiveValue::Set(app_id),
        build_id: ActiveValue::Set("b1".into()),
        s3_prefix: ActiveValue::Set("apps/b1".into()),
        manifest_json: ActiveValue::Set(Some(json!({ "functions": {
            "refresh-qb-token": { "webhook": { "secretVar": "QB_HOOK_A, QB_HOOK_B" } } } }))),
        created_at: ActiveValue::Set(chrono::Utc::now().fixed_offset()),
        validation_status: ActiveValue::Set("ok".into()),
        ..Default::default()
    }
    .insert(&fx.db)
    .await
    .expect("seed build");
}

/// BLOCKING (save): Pokehouse's real rotator is a custom app's Function, whose
/// grant lives at `apps/<app_id>/…` and is named in no pipeline YAML. Any
/// app-scoped var (`/`), and any key a custom app in the workspace declares,
/// is refused as `409 reserved_var`; nothing is stored.
#[tokio::test]
async fn save_refuses_an_app_scoped_or_app_declared_var() {
    let fx = world().await;
    seed_app(&fx).await;
    let eastbay = "quickbooks_financials_eastbay";
    let app_token = format!("apps/{}/QB_REFRESH_TOKEN_EASTBAY", Uuid::new_v4());
    let with = |key: &str, value: &str| {
        let mut o = good();
        o[key] = json!(value);
        o
    };
    for overrides in [
        with("refresh_token_var", &app_token),
        with("client_secret_var", "apps/x/QB_CLIENT_SECRET"),
        with("refresh_token_var", "QB_REFRESH_TOKEN_EASTBAY"),
        with("client_id_var", "QB_HOOK_B"),
        json!({ "realm_id": "4620816365000000", "access_token_var": app_token.clone() }),
    ] {
        let (status, body) = put(&fx, sandbox(eastbay, overrides.clone())).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::CONFLICT, Some("reserved_var")),
            "{overrides}: {body}"
        );
    }
    let (_, listed) = send_json(
        &fx.staff,
        "GET",
        format!("/{}/previews/sources", fx.ws),
        None,
    )
    .await;
    assert_eq!(listed, json!([]), "nothing refused was stored");
    let (status, body) = put(&fx, sandbox(eastbay, good())).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the control: the sandbox's own names: {body}"
    );
}
