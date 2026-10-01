//! The checks API's fixture: a workspace whose promoted revision serves one
//! Airway pipeline and whose previewed branch edits it and adds another, plus
//! the staff and customer callers and a router over the previews handlers.

use axum::body::Body;
use axum::extract::Extension;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::{Router, middleware};
use entity::users::UserStatus;
use entity::workspaces::WorkspaceStatus;
use entity::{airway_pipelines, organizations, revisions, users, workspaces};
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement,
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

pub(crate) const BRANCH: &str = "feat/je-v2";
pub(crate) const FEAT_SHA: &str = "feedfacefeedfacefeedfacefeedfacefeedface";
pub(crate) const STAFF: &str = "staff@oxy.test";

pub(crate) struct Fx {
    pub db: DatabaseConnection,
    pub ws: Uuid,
    pub staff: AuthenticatedUser,
    pub customer: AuthenticatedUser,
    pub staging: Uuid,
}

pub(crate) async fn user(db: &DatabaseConnection, org: Uuid, email: &str) -> AuthenticatedUser {
    let id = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(Some(email.into())),
        name: ActiveValue::Set(format!("Name of {email}")),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed user");
    entity::org_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(org),
        user_id: ActiveValue::Set(id),
        role: ActiveValue::Set(entity::org_members::OrgRole::Admin),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed membership");
    AuthenticatedUser {
        id,
        email: Some(email.into()),
        name: email.into(),
        picture: None,
        status: UserStatus::Active,
    }
}

pub(crate) async fn revision(db: &DatabaseConnection, ws: Uuid, sha: &str, kind: &str) -> Uuid {
    let now = chrono::Utc::now().fixed_offset();
    let id = Uuid::new_v4();
    revisions::ActiveModel {
        revision_id: ActiveValue::Set(id),
        workspace_id: ActiveValue::Set(ws),
        git_sha: ActiveValue::Set(sha.into()),
        branch: ActiveValue::Set(Some(if kind == "main" { "main" } else { BRANCH }.into())),
        schema_version: ActiveValue::Set(oxy_compile::CURRENT_SCHEMA_VERSION),
        status: ActiveValue::Set("ready".into()),
        kind: ActiveValue::Set(kind.into()),
        owner_user_id: ActiveValue::Set(None),
        compiler_version: ActiveValue::Set(oxy_compile::compiler_version()),
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
    id
}

pub(crate) async fn pipeline(db: &DatabaseConnection, rev: Uuid, path: &str, def: Value) {
    airway_pipelines::ActiveModel {
        revision_id: ActiveValue::Set(rev),
        name: ActiveValue::Set(def["name"].as_str().unwrap().into()),
        file_path: ActiveValue::Set(path.into()),
        definition: ActiveValue::Set(def),
    }
    .insert(db)
    .await
    .expect("seed pipeline");
}

pub(crate) async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .await
    .expect(sql);
}

/// NCES schools over `rest_api`, landing in the managed Airhouse; the branch
/// turns `replace` into `merge` on a key — a change Airway cannot absorb.
pub(crate) fn nces(disposition: &str) -> Value {
    let mut endpoint = json!({
        "name": "schools",
        "path": "/schools/ccd/directory/2022/",
        "data_path": "results",
        "write_disposition": disposition,
    });
    if disposition == "merge" {
        endpoint["primary_key"] = json!(["ncessch"]);
    }
    json!({
        "name": "nces_schools",
        "source": {
            "kind": "rest_api",
            "config": { "base_url": "https://educationdata.urban.org/api/v1", "endpoints": [endpoint] },
        },
        "destination": { "database": "airhouse", "dataset_name": "nces" },
    })
}

/// Main serves `nces_schools` (loaded once, as `replace`); the branch edits it
/// and adds a pipeline whose source this server cannot build.
pub(crate) async fn setup() -> Fx {
    let db = common_db().await;
    let now = chrono::Utc::now().fixed_offset();
    let org = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org),
        name: ActiveValue::Set("acme".into()),
        slug: ActiveValue::Set(format!("acme-{}", org.simple())),
        logo: ActiveValue::NotSet,
        logo_content_type: ActiveValue::NotSet,
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(&db)
    .await
    .expect("seed org");
    let ws = workspaces::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        name: ActiveValue::Set("ws".into()),
        org_id: ActiveValue::Set(Some(org)),
        status: ActiveValue::Set(WorkspaceStatus::Ready),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed workspace")
    .id;
    let staff = user(&db, org, STAFF).await;
    let customer = user(&db, org, "admin@acme.test").await;

    let main = revision(&db, ws, "main-sha", "main").await;
    pipeline(&db, main, "airway/nces.airway.yml", nces("replace")).await;
    exec(
        &db,
        "INSERT INTO workspace_compiled_configs (revision_id, databases) VALUES ($1, $2)",
        vec![
            main.into(),
            json!([{ "name": "airhouse", "type": "airhouse_managed" }]).into(),
        ],
    )
    .await;
    exec(
        &db,
        "UPDATE workspaces SET current_revision_id = $1 WHERE id = $2",
        vec![main.into(), ws.into()],
    )
    .await;
    exec(
        &db,
        "INSERT INTO airway_workspace_pipeline_state (workspace_id, pipeline_name, state, schema_json) \
         VALUES ($1, 'nces_schools', '{}'::jsonb, $2)",
        vec![
            ws.into(),
            json!({
                "name": "nces_schools", "version": 1, "version_hash": "", "engine_version": 1,
                "tables": { "schools": { "name": "schools", "columns": {}, "write_disposition": "replace" } },
            })
            .into(),
        ],
    )
    .await;

    let staging = revision(&db, ws, FEAT_SHA, "staging").await;
    pipeline(&db, staging, "airway/nces.airway.yml", nces("merge")).await;
    pipeline(
        &db,
        staging,
        "airway/zz_unknown.airway.yml",
        json!({
            "name": "mystery",
            "source": { "kind": "no_such_source", "config": {} },
            "destination": { "database": "airhouse", "dataset_name": "mystery" },
        }),
    )
    .await;
    // A ready revision always has its compiled config (the compile writes it).
    exec(
        &db,
        "INSERT INTO workspace_compiled_configs (revision_id, databases) VALUES ($1, $2)",
        vec![
            staging.into(),
            json!([{ "name": "airhouse", "type": "airhouse_managed" }]).into(),
        ],
    )
    .await;
    exec(
        &db,
        "INSERT INTO workspace_previews (workspace_id, branch, git_sha, created_by) VALUES ($1, $2, $3, $4)",
        vec![ws.into(), BRANCH.into(), FEAT_SHA.into(), staff.id.into()],
    )
    .await;
    Fx {
        db,
        ws,
        staff,
        customer,
        staging,
    }
}

pub(crate) async fn common_db() -> DatabaseConnection {
    let db = crate::common::test_db_with(crate::common::Schema::All).await;
    // SAFETY: nextest runs each test in its own process.
    unsafe {
        std::env::set_var("OXY_OWNER", STAFF);
        // No Airhouse here, whatever the developer's shell says.
        for var in [
            "AIRHOUSE_WIRE_HOST",
            "AIRHOUSE_ANALYTICS_WIRE_HOST",
            "AIRHOUSE_BASE_URL",
            "AIRHOUSE_ADMIN_TOKEN",
        ] {
            std::env::remove_var(var);
        }
    }
    db
}

pub(crate) fn api(user: AuthenticatedUser) -> Router {
    use oxy_app::server::api::middlewares::workspace_context::workspace_access_middleware;
    use oxy_app::server::api::workspace_previews as h;
    let previews = Router::new()
        .route("/", get(h::list_previews))
        .route("/checks", get(h::get_checks))
        .route("/runs", axum::routing::post(h::start_run).get(h::list_runs))
        .route("/runs/{run_id}", get(h::get_run))
        .route("/sources", get(h::list_sources).put(h::put_source))
        .layer(middleware::from_fn(workspace_access_middleware));
    Router::new()
        .nest("/{workspace_id}/previews", previews)
        .layer(Extension(user))
}

pub(crate) async fn get_json(user: &AuthenticatedUser, uri: String) -> (StatusCode, Value) {
    send_json(user, "GET", uri, None).await
}

/// `method uri` as `user`, with an optional JSON body; the answer as JSON.
pub(crate) async fn send_json(
    user: &AuthenticatedUser,
    method: &str,
    uri: String,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let request = Request::builder().method(method).uri(uri);
    let request = match body {
        Some(b) => request
            .header("content-type", "application/json")
            .body(Body::from(b.to_string())),
        None => request.body(Body::empty()),
    };
    let resp = api(user.clone()).oneshot(request.unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 256 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
