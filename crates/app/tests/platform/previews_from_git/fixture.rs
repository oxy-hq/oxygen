//! A `serve` process, its workspace and a staff caller, behind the real
//! preview and staging handlers.

use axum::body::Body;
use axum::extract::Extension;
use axum::http::{Request, StatusCode};
use axum::routing::{get, post};
use axum::{Router, middleware};
use entity::users::UserStatus;
use entity::{organizations, users, workspace_preview_runs, workspace_previews};
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use crate::compile_request::fixture::Fx;

pub const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
pub const MOVED: &str = "89abcdef0123456789abcdef0123456789abcdef";
pub const BRANCH: &str = "feat/x";
const STAFF: &str = "staff@oxy.test";

pub struct World {
    pub fx: Fx,
    staff: AuthenticatedUser,
}

impl World {
    /// A `serve` process, a remote-backed workspace in an org, and a staff
    /// member of it. `factory` is the upstream the replica may replay to.
    pub async fn new(factory: Option<&str>) -> Self {
        let fx = Fx::new(Some("serve")).await;
        // After the database: `test_db_with` clears `OXY_OWNER`.
        // SAFETY: nextest runs each test in its own process, and nothing has
        // read these yet (`ide_upstream` reads its variable once).
        unsafe {
            std::env::set_var("OXY_OWNER", STAFF);
            match factory {
                Some(url) => std::env::set_var("OXY_IDE_UPSTREAM", url),
                None => std::env::remove_var("OXY_IDE_UPSTREAM"),
            }
        }
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
        .insert(&fx.db)
        .await
        .expect("seed org");
        fx.edit_workspace(|w| w.org_id = ActiveValue::Set(Some(org)))
            .await;
        let staff = staff_member(&fx, org).await;
        Self { fx, staff }
    }

    pub async fn send(&self, method: &str, uri: String, body: Option<Value>) -> Answer {
        let mut request = Request::builder().method(method).uri(uri);
        let body = match body {
            Some(b) => {
                request = request.header("content-type", "application/json");
                Body::from(b.to_string())
            }
            None => Body::empty(),
        };
        let response = api(self.staff.clone())
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        Answer {
            status,
            headers,
            text: String::from_utf8_lossy(&bytes).into_owned(),
        }
    }

    pub fn previews(&self) -> String {
        format!("/api/{}/previews", self.fx.ws)
    }

    pub async fn create(&self, branch: &str) -> Answer {
        self.send("POST", self.previews(), Some(json!({ "branch": branch })))
            .await
    }

    pub async fn refresh(&self, branch: &str) -> Answer {
        let uri = format!("{}/refresh?branch={}", self.previews(), encode(branch));
        self.send("POST", uri, None).await
    }

    /// The listed preview of `branch`.
    pub async fn listed(&self, branch: &str) -> Value {
        let list = self.send("GET", self.previews(), None).await;
        assert_eq!(list.status, StatusCode::OK, "{}", list.text);
        list.json()["items"]
            .as_array()
            .expect("items")
            .iter()
            .find(|item| item["branch"] == branch)
            .cloned()
            .unwrap_or_else(|| panic!("no preview of {branch}: {}", list.text))
    }

    pub async fn stage(&self, branch: &str) -> Answer {
        let uri = format!(
            "/api/{}/compile/staging?branch={}",
            self.fx.ws,
            encode(branch)
        );
        self.send("POST", uri, None).await
    }

    pub async fn preview_rows(&self) -> Vec<workspace_previews::Model> {
        workspace_previews::Entity::find()
            .filter(workspace_previews::Column::WorkspaceId.eq(self.fx.ws))
            .all(&self.fx.db)
            .await
            .expect("read previews")
    }

    /// The revisions a change check has been queued for.
    pub async fn checks_queued(&self) -> Vec<Uuid> {
        workspace_preview_runs::Entity::find()
            .filter(workspace_preview_runs::Column::WorkspaceId.eq(self.fx.ws))
            .filter(workspace_preview_runs::Column::Kind.eq("analyze"))
            .all(&self.fx.db)
            .await
            .expect("read preview runs")
            .into_iter()
            .map(|run| run.revision_id)
            .collect()
    }
}

pub struct Answer {
    pub status: StatusCode,
    pub headers: axum::http::HeaderMap,
    pub text: String,
}

impl Answer {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or_else(|_| panic!("not JSON: {}", self.text))
    }
}

pub fn encode(branch: &str) -> String {
    branch.replace('/', "%2F")
}

async fn staff_member(fx: &Fx, org: Uuid) -> AuthenticatedUser {
    let id = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(Some(STAFF.into())),
        name: ActiveValue::Set("Staff".into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        ..Default::default()
    }
    .insert(&fx.db)
    .await
    .expect("seed staff");
    entity::org_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(org),
        user_id: ActiveValue::Set(id),
        role: ActiveValue::Set(entity::org_members::OrgRole::Admin),
        ..Default::default()
    }
    .insert(&fx.db)
    .await
    .expect("seed membership");
    AuthenticatedUser {
        id,
        email: Some(STAFF.into()),
        name: "Staff".into(),
        picture: None,
        status: UserStatus::Active,
        credential: None,
    }
}

/// The handlers as the product mounts them, minus what they do not read: the
/// previews nest behind the access check alone, as in `router::protected`.
fn api(user: AuthenticatedUser) -> Router {
    use oxy_app::server::api::middlewares::workspace_context::workspace_access_middleware;
    use oxy_app::server::api::{compile_staging as staging, workspace_previews as previews};
    let previews = Router::new()
        .route(
            "/",
            post(previews::create_preview)
                .get(previews::list_previews)
                .delete(previews::delete_preview),
        )
        .route("/refresh", post(previews::refresh_preview))
        .route("/checks", get(previews::get_checks))
        .layer(middleware::from_fn(workspace_access_middleware));
    let compile = Router::new()
        .route("/compile/staging", post(staging::enqueue_staging_compile))
        .route(
            "/compile/staging/status",
            get(staging::staging_compile_status),
        )
        .layer(middleware::from_fn(workspace_access_middleware));
    Router::new()
        .nest("/api/{workspace_id}/previews", previews)
        .nest("/api/{workspace_id}", compile)
        .layer(Extension(user))
}
