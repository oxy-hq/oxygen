//! Two tenants, an org-less workspace, and four callers — the cast every
//! `admin_staff_scope` case asks its question of.

use axum::body::to_bytes;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use entity::users::UserStatus;
use entity::workspaces::WorkspaceStatus;
use entity::{app_admin_scope_orgs, app_admins, organizations, users, workspaces};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection};
use serde_json::Value;
use uuid::Uuid;

use crate::common::{Schema, test_db_with};

/// The address `OXY_OWNER` names for the duration of a test.
const OWNER_EMAIL: &str = "root@staff-scope.test";

pub struct World {
    pub db: DatabaseConnection,
    /// The org the bounded grant names, and its workspace.
    pub org_a: Uuid,
    pub ws_a: Uuid,
    /// The org it does not, and its workspace.
    pub org_b: Uuid,
    pub ws_b: Uuid,
    /// A workspace that belongs to no org — a platform-level row.
    pub ws_orphan: Uuid,
    /// `global_admin`, bounded to `org_a`. The caller the finding is about.
    pub bounded: AuthenticatedUser,
    /// `global_admin`, `scope_all`. Must see exactly what it saw before.
    pub unbounded: AuthenticatedUser,
    /// The Global Owner: in `OXY_OWNER`, holding no grant row.
    pub owner: AuthenticatedUser,
}

impl World {
    /// The callers who must reach everything, with a label for assertions.
    pub fn everything_readers(&self) -> [(&'static str, &AuthenticatedUser); 2] {
        [
            ("an all-orgs grant", &self.unbounded),
            ("the Global Owner", &self.owner),
        ]
    }
}

/// What a handler answered, as an HTTP client would see it.
pub struct Reply {
    pub status: StatusCode,
    /// The `Link` header — carries `rel="next"` when a paged listing has more.
    pub link: String,
    pub body: Value,
}

impl Reply {
    /// Every value of `key` across the rows of a JSON-array body (or of `rows_key`
    /// inside an object body).
    pub fn column(&self, rows_key: Option<&str>, key: &str) -> Vec<String> {
        let rows = match rows_key {
            Some(k) => &self.body[k],
            None => &self.body,
        };
        rows.as_array()
            .unwrap_or_else(|| panic!("expected a row array, got {}", self.body))
            .iter()
            .map(|row| match &row[key] {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect()
    }

    pub fn has_next(&self) -> bool {
        self.link.contains("rel=\"next\"")
    }
}

/// Drive a handler's return value the whole way to bytes, so a case asserts on the
/// response a client receives rather than on a DTO's private fields.
pub async fn reply(answer: impl IntoResponse) -> Reply {
    let response = answer.into_response();
    let status = response.status();
    let link = response
        .headers()
        .get(axum::http::header::LINK)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read response body");
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply { status, link, body }
}

pub fn as_actor(user: &AuthenticatedUser) -> AuthenticatedUserExtractor {
    AuthenticatedUserExtractor(user.clone())
}

pub async fn seed_org(db: &DatabaseConnection, tag: &str) -> Uuid {
    let id = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(id),
        name: ActiveValue::Set(format!("Scope {tag} {id}")),
        slug: ActiveValue::Set(format!("scope-{tag}-{id}")),
        logo: ActiveValue::NotSet,
        logo_content_type: ActiveValue::NotSet,
        created_at: ActiveValue::NotSet,
        updated_at: ActiveValue::NotSet,
    }
    .insert(db)
    .await
    .expect("seed org");
    id
}

/// A workspace with a `path` and no promoted revision — so it is also a candidate
/// for the compile backfill.
pub async fn seed_workspace(db: &DatabaseConnection, tag: &str, org: Option<Uuid>) -> Uuid {
    let id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(id),
        name: ActiveValue::Set(format!("ws-{tag}")),
        org_id: ActiveValue::Set(org),
        path: ActiveValue::Set(Some(format!("/nonexistent/{id}"))),
        status: ActiveValue::Set(WorkspaceStatus::Ready),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed workspace");
    id
}

pub async fn seed_user(db: &DatabaseConnection, email: &str) -> AuthenticatedUser {
    let id = Uuid::new_v4();
    let user = users::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(Some(email.to_string())),
        name: ActiveValue::Set(format!("Staff {email}")),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        magic_link_token: ActiveValue::Set(None),
        magic_link_token_expires_at: ActiveValue::Set(None),
        status: ActiveValue::Set(UserStatus::Active),
        created_at: ActiveValue::NotSet,
        last_login_at: ActiveValue::NotSet,
    }
    .insert(db)
    .await
    .expect("seed user");
    AuthenticatedUser {
        id: user.id,
        email: user.email,
        name: user.name,
        picture: user.picture,
        status: user.status,
        credential: None,
    }
}

/// A `global_admin` grant row: `orgs = None` is `scope_all`, `Some(..)` is bounded
/// to exactly those orgs (an empty list reaches nothing).
pub async fn grant(db: &DatabaseConnection, email: &str, orgs: Option<&[Uuid]>) {
    let id = Uuid::new_v4();
    app_admins::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(email.to_ascii_lowercase()),
        granted_by: ActiveValue::Set(None),
        created_at: ActiveValue::NotSet,
        role: ActiveValue::Set("global_admin".to_string()),
        scope_all: ActiveValue::Set(orgs.is_none()),
        updated_at: ActiveValue::NotSet,
    }
    .insert(db)
    .await
    .expect("seed grant");
    for org in orgs.unwrap_or_default() {
        app_admin_scope_orgs::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            app_admin_id: ActiveValue::Set(id),
            org_id: ActiveValue::Set(*org),
            created_at: ActiveValue::NotSet,
            created_by: ActiveValue::Set(None),
        }
        .insert(db)
        .await
        .expect("seed grant scope");
    }
}

/// A database of its own with the process pointed at it, two tenants, an org-less
/// workspace, and the three staff callers.
pub async fn world() -> World {
    let db = test_db_with(Schema::All).await;
    // SAFETY: nextest runs each test in its own process (`test_db_with` asserts it),
    // and this happens before any handler reads the environment. `test_db_with`
    // clears `OXY_OWNER` so a developer's shell cannot make a test user root; this
    // names the one address these cases mean by "the Global Owner".
    unsafe {
        std::env::set_var("OXY_OWNER", OWNER_EMAIL);
    }

    let org_a = seed_org(&db, "a").await;
    let org_b = seed_org(&db, "b").await;
    let ws_a = seed_workspace(&db, "a", Some(org_a)).await;
    let ws_b = seed_workspace(&db, "b", Some(org_b)).await;
    let ws_orphan = seed_workspace(&db, "orphan", None).await;

    let bounded = seed_user(&db, "bounded@staff-scope.test").await;
    grant(&db, "bounded@staff-scope.test", Some(&[org_a])).await;
    let unbounded = seed_user(&db, "unbounded@staff-scope.test").await;
    grant(&db, "unbounded@staff-scope.test", None).await;
    let owner = seed_user(&db, OWNER_EMAIL).await;

    World {
        db,
        org_a,
        ws_a,
        org_b,
        ws_b,
        ws_orphan,
        bounded,
        unbounded,
        owner,
    }
}
