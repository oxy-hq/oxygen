//! The cast and the props: a database of its own, seeds for every table the
//! report reads, and a mail provider that keeps what it is given.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use entity::{
    app_admin_scope_orgs, app_admins, app_builds, app_environment_events, app_function_invocations,
    app_storage_usage_samples, apps, custom_app_event, custom_app_view_event, organizations, users,
};
use oxy_app::emails::{EmailMessage, EmailProvider};
use oxy_app::server::api::admin::usage_report::period::Period;
use oxy_shared::errors::OxyError;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ConnectionTrait, DatabaseConnection, DbBackend, Statement,
};
use serde_json::json;
use uuid::Uuid;

/// The instant every case treats as "now": two hours into the Monday after the
/// last completed week — past the hour the pass waits before writing a report,
/// and inside the day it mails one — whatever day the suite runs on.
pub fn now() -> DateTime<Utc> {
    Period::last_completed(Utc::now()).end + Duration::hours(2)
}

pub struct World {
    pub db: DatabaseConnection,
    pub period: Period,
    /// The build each app's invocations hang off.
    builds: Mutex<HashMap<Uuid, Uuid>>,
}

impl World {
    pub async fn new() -> Self {
        let db = crate::common::fresh_db(crate::common::Schema::Central)
            .await
            .0;
        Self {
            db,
            period: Period::last_completed(now()),
            builds: Mutex::new(HashMap::new()),
        }
    }

    /// `hours` into the reported week, or into the week before it.
    pub fn this_week(&self, hours: i64) -> DateTime<Utc> {
        self.period.start + Duration::hours(hours)
    }

    pub fn week_before(&self, hours: i64) -> DateTime<Utc> {
        self.period.previous().start + Duration::hours(hours)
    }

    pub async fn org(&self, name: &str) -> Uuid {
        let id = Uuid::new_v4();
        organizations::ActiveModel {
            id: Set(id),
            name: Set(name.to_string()),
            slug: Set(format!(
                "{}-{}",
                name.to_lowercase().replace(' ', "-"),
                id.simple()
            )),
            ..Default::default()
        }
        .insert(&self.db)
        .await
        .expect("seed org");
        id
    }

    pub async fn user(&self, email: &str) -> Uuid {
        let id = Uuid::new_v4();
        users::ActiveModel {
            id: Set(id),
            email: Set(Some(email.to_string())),
            name: Set(email.to_string()),
            email_verified: Set(true),
            status: Set(users::UserStatus::Active),
            ..Default::default()
        }
        .insert(&self.db)
        .await
        .expect("seed user");
        id
    }

    pub async fn app(&self, org: Uuid, name: &str, published: bool) -> Uuid {
        let id = Uuid::new_v4();
        apps::ActiveModel {
            id: Set(id),
            org_id: Set(org),
            project_id: Set(Uuid::new_v4()),
            slug: Set(format!(
                "{}-{}",
                name.to_lowercase().replace(' ', "-"),
                id.simple()
            )),
            name: Set(name.to_string()),
            branch: Set("main".into()),
            source_repo: Set("git@example.com:acme/app.git".into()),
            status: Set("active".into()),
            source_type: Set("git".into()),
            source_config: Set(json!({})),
            visibility: Set("org".into()),
            published_at: Set(published.then(|| self.week_before(-24 * 60).fixed_offset())),
            ..Default::default()
        }
        .insert(&self.db)
        .await
        .expect("seed app");
        let build = Uuid::new_v4();
        app_builds::ActiveModel {
            id: Set(build),
            app_id: Set(id),
            build_id: Set("b1".into()),
            s3_prefix: Set("apps/b1".into()),
            created_at: Set(Utc::now().fixed_offset()),
            validation_status: Set("ok".into()),
            ..Default::default()
        }
        .insert(&self.db)
        .await
        .expect("seed build");
        self.builds.lock().unwrap().insert(id, build);
        id
    }

    pub async fn view(&self, app: Uuid, user: Uuid, at: DateTime<Utc>, environment: &str) -> Uuid {
        let session = Uuid::new_v4();
        custom_app_view_event::ActiveModel {
            id: Set(Uuid::new_v4()),
            app_id: Set(app),
            user_id: Set(user),
            user_email: Set("someone@customer.test".into()),
            session_id: Set(session),
            viewed_at: Set(at.fixed_offset()),
            user_agent_class: Set("browser".into()),
            source: Set("subpath".into()),
            environment: Set(environment.into()),
            ..Default::default()
        }
        .insert(&self.db)
        .await
        .expect("seed view");
        session
    }

    pub async fn event(&self, app: Uuid, user: Uuid, session: Uuid, name: &str, at: DateTime<Utc>) {
        custom_app_event::ActiveModel {
            id: Set(Uuid::new_v4()),
            app_id: Set(app),
            user_id: Set(user),
            user_email: Set("someone@customer.test".into()),
            session_id: Set(session),
            event_name: Set(name.into()),
            payload: Set(json!({})),
            occurred_at: Set(at.fixed_offset()),
            environment: Set("production".into()),
        }
        .insert(&self.db)
        .await
        .expect("seed event");
    }

    pub async fn call(&self, app: Uuid, status: &str, at: DateTime<Utc>, environment: &str) {
        let build = self.builds.lock().unwrap()[&app];
        app_function_invocations::ActiveModel {
            id: Set(Uuid::new_v4()),
            app_id: Set(app),
            build_id: Set(build),
            function_name: Set("sync".into()),
            mode: Set("route".into()),
            status: Set(status.into()),
            created_at: Set(at.fixed_offset()),
            environment: Set(environment.into()),
            // Every other column is nullable; leaving them unset keeps this
            // seed compiling when the table gains one.
            ..Default::default()
        }
        .insert(&self.db)
        .await
        .expect("seed invocation");
    }

    /// A build moving in one of the app's environments.
    pub async fn release(&self, app: Uuid, environment: &str, action: &str, at: DateTime<Utc>) {
        app_environment_events::ActiveModel {
            id: Set(Uuid::new_v4()),
            app_id: Set(app),
            environment: Set(environment.into()),
            build_id: Set(None),
            action: Set(action.into()),
            actor: Set(None),
            at: Set(at.fixed_offset()),
        }
        .insert(&self.db)
        .await
        .expect("seed release");
    }

    /// What the storage sweeper measured for an app at `at`.
    pub async fn stored(&self, app: Uuid, bytes: i64, at: DateTime<Utc>) {
        app_storage_usage_samples::ActiveModel {
            app_id: Set(app),
            measured_at: Set(at.fixed_offset()),
            bytes: Set(bytes),
            object_count: Set(1),
        }
        .insert(&self.db)
        .await
        .expect("seed storage sample");
    }

    /// A platform grant: `orgs = None` reaches every org.
    pub async fn grant(&self, email: &str, role: &str, orgs: Option<&[Uuid]>) {
        let id = Uuid::new_v4();
        app_admins::ActiveModel {
            id: Set(id),
            email: Set(email.to_string()),
            granted_by: Set(None),
            role: Set(role.to_string()),
            scope_all: Set(orgs.is_none()),
            ..Default::default()
        }
        .insert(&self.db)
        .await
        .expect("seed grant");
        for org in orgs.unwrap_or_default() {
            app_admin_scope_orgs::ActiveModel {
                id: Set(Uuid::new_v4()),
                app_admin_id: Set(id),
                org_id: Set(*org),
                created_by: Set(None),
                ..Default::default()
            }
            .insert(&self.db)
            .await
            .expect("seed grant scope");
        }
    }

    pub async fn count(&self, table: &str) -> i64 {
        self.db
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                format!("SELECT COUNT(*)::bigint AS n FROM {table}"),
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "n")
            .unwrap()
    }
}

/// Keeps every mail it is given; refuses the addresses it was told to, once.
#[derive(Default)]
pub struct Outbox {
    pub sent: Mutex<Vec<(String, EmailMessage)>>,
    pub refuse_once: Mutex<Vec<String>>,
}

#[async_trait]
impl EmailProvider for Outbox {
    async fn send(&self, _from: &str, to: &str, message: EmailMessage) -> Result<(), OxyError> {
        let mut refuse = self.refuse_once.lock().unwrap();
        if let Some(at) = refuse.iter().position(|e| e == to) {
            refuse.remove(at);
            return Err(OxyError::RuntimeError("the provider said no".into()));
        }
        self.sent.lock().unwrap().push((to.to_string(), message));
        Ok(())
    }
}

impl Outbox {
    pub fn to(&self) -> Vec<String> {
        let mut to: Vec<String> = self
            .sent
            .lock()
            .unwrap()
            .iter()
            .map(|(t, _)| t.clone())
            .collect();
        to.sort();
        to
    }

    pub fn text_for(&self, address: &str) -> String {
        let sent = self.sent.lock().unwrap();
        let (_, message) = sent
            .iter()
            .find(|(to, _)| to == address)
            .unwrap_or_else(|| panic!("nothing was sent to {address}"));
        message.text_body.clone()
    }
}
