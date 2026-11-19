//! The staff endpoints over app events (design §6.8), the routes `oxyc events`
//! calls (`sdk/cli/src/commands/events.ts`), driven through the router
//! `public_routes()` mounts, as the guest.
//!
//! Events and deliveries are stored through `fan_out`, the platform half of
//! the relay, so a production delivery is `pending` with a real task row; its
//! status, attempts and ages are then set directly. These orgs have no OLTP
//! store: an event's body reads as unavailable, a replay finds no body. The
//! `store` module runs the routes that read or delete bodies against an org
//! with a store of its own (`app_events_delivery`'s fixture).
//!
//! | Module | Proves |
//! | --- | --- |
//! | `reads` | `types` lists each type with its publisher, schema revision and last publish, and per listener and environment its health (counts, the oldest pending age, last success and failure, in flight) and where its subscription came from; an event shows its deliveries with every attempt and its causation both ways; `find` derives the id from the key; `?root=` lists the chain in publish order; `feed` lists publishes and deliveries after `since`; `dead` groups by outcome and the error's first line |
//! | `redeliver` | a dead delivery is queued again on the same delivery and event; a dry run writes nothing and counts; a bulk redeliver spreads its tasks at 10 a second per listener; a redelivered expired delivery is not expired again by the next sweep |
//! | `destructive` | discard and purge write an audit row; the switch is refused for production and flips staging; a caller who is neither staff nor an org officer is refused every route; every route that changes something is `409 events_not_enabled` while the org's flag is off, and the reads still answer |
//! | `contract` | each reply carries every key the CLI reads |
//! | `store` | an event shows its body read from the org's store; a redelivered dead delivery runs again with a fresh budget of attempts; replay creates deliveries only for retained events whose body is still kept; purge deletes the bodies too, and refuses, deleting nothing, when the store cannot be reached |

mod contract;
mod destructive;
mod reads;
mod redeliver;
mod store;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use chrono::{DateTime, Utc};
use entity::{app_event_deliveries as deliveries, workspaces};
use oxy_api_custom_apps::app_events::relay::RelayedEvent;
use oxy_api_custom_apps::app_events::relay::testing::fan_out;
use oxy_api_custom_apps::custom_apps_publish::{OrgRef, PublishInput, publish};
use oxy_api_custom_apps::public_routes;
use oxy_app::surface::bare_app_state;
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use oxy_custom_apps::events::{OxyEvent, event_id, qualified_type};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseBackend,
    DatabaseConnection, EntityTrait, QueryFilter, Statement,
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use crate::custom_app_functions_fixture::{FunctionSpec, Tenant, seeded_tenant, throwaway_org};

pub(super) const EVENT: &str = "statement.uploaded";
pub(super) const FUNCTION: &str = "on-event";
const JS: &str = "export default async () => Response.json({});";

fn schema() -> Value {
    json!({ "type": "object", "properties": { "period": { "type": "string" } } })
}

/// An org with a workspace, a publisher declaring [`EVENT`] and a listener
/// whose [`FUNCTION`] listens to it, both promoted to production.
pub(super) struct Fix {
    pub(super) t: Tenant,
    pub(super) workspace: Uuid,
    pub(super) publisher: Uuid,
    pub(super) publisher_slug: String,
    pub(super) listener: Uuid,
    pub(super) listener_slug: String,
    _root: tempfile::TempDir,
}

/// Where an event sits in a causation chain: its parent, its root, its depth.
#[derive(Clone, Copy)]
pub(super) struct Chain {
    pub(super) parent: Uuid,
    pub(super) root: Uuid,
    pub(super) depth: i32,
}

impl Fix {
    /// With app events on for the org.
    pub(super) async fn new() -> Self {
        Self::with_flag(true).await
    }

    pub(super) async fn with_flag(on: bool) -> Self {
        let t = throwaway_org(&seeded_tenant().await).await;
        if on {
            crate::app_events_publish::enable_app_events(&t, &[t.org_id]).await;
        }
        let root = tempfile::tempdir().expect("workspace dir");
        let workspace = Uuid::new_v4();
        workspaces::ActiveModel {
            id: Set(workspace),
            name: Set("Events API".into()),
            org_id: Set(Some(t.org_id)),
            path: Set(Some(root.path().to_string_lossy().into_owned())),
            ..Default::default()
        }
        .insert(&t.db)
        .await
        .expect("seed the workspace");
        let tag = Uuid::new_v4().simple().to_string()[..8].to_string();
        let publisher_slug = format!("api-pub-{tag}");
        let declares = json!({ "events": { EVENT: { "schema": schema() } } });
        let publisher = publish_app(&t, &publisher_slug, workspace, &[], declares).await;
        let function = FunctionSpec {
            name: FUNCTION,
            manifest: json!({ "on": [qualified_type(&publisher_slug, EVENT)] }),
            js: JS,
        };
        let listener_slug = format!("api-lis-{tag}");
        let listener = publish_app(&t, &listener_slug, workspace, &[function], json!({})).await;
        Self {
            t,
            workspace,
            publisher,
            publisher_slug,
            listener,
            listener_slug,
            _root: root,
        }
    }

    pub(super) fn db(&self) -> &DatabaseConnection {
        &self.t.db
    }

    pub(super) fn org(&self) -> &str {
        &self.t.org_slug
    }

    pub(super) fn event_type(&self) -> String {
        qualified_type(&self.publisher_slug, EVENT)
    }

    /// The listener as `oxyc events` names it: `<app>/<function>`.
    pub(super) fn to(&self) -> String {
        format!("{}/{FUNCTION}", self.listener_slug)
    }

    /// Store a production event `key` (a root) and fan it out: one `pending`
    /// delivery to the listener, with its task queued. Its id.
    pub(super) async fn publish(&self, key: &str) -> Uuid {
        self.publish_in(key, None).await
    }

    /// [`Self::publish`], the event at `chain` in a causation chain.
    pub(super) async fn publish_in(&self, key: &str, chain: Option<Chain>) -> Uuid {
        let event_type = self.event_type();
        let id = event_id(self.t.org_id, "production", &event_type, key);
        let event = RelayedEvent {
            envelope: OxyEvent {
                specversion: "1.0".into(),
                id,
                dataschema: format!("{event_type}@1"),
                event_type,
                source: format!("/apps/{}", self.publisher_slug),
                time: Utc::now(),
                data: Value::Null,
                key: key.into(),
                environment: "production".into(),
                invocation: None,
                actor: None,
                causationid: chain.map(|c| c.parent),
                rootid: chain.map_or(id, |c| c.root),
                depth: chain.map_or(0, |c| c.depth),
            },
            org_id: self.t.org_id,
            workspace_id: self.workspace,
            source_app_id: Some(self.publisher),
            // This org has no OLTP store.
            writer_role: "app_writer_x".into(),
            data_bytes: 21,
            schema_revision_id: None,
            delay_seconds: 0,
            relay_error: None,
            directed: None,
            only_listener: None,
            invoker: None,
        };
        fan_out(self.db(), &[event]).await.expect("fan out");
        id
    }

    /// The listener's delivery of `event`.
    pub(super) async fn delivery(&self, event: Uuid) -> deliveries::Model {
        deliveries::Entity::find()
            .filter(deliveries::Column::EventId.eq(event))
            .one(self.db())
            .await
            .expect("read the delivery")
            .expect("the event's delivery")
    }

    pub(super) async fn run(&self, sql: &str, args: Vec<sea_orm::Value>) {
        run_sql(self.db(), sql, args).await;
    }

    /// Set `event`'s delivery to `status` with `attempts` begun.
    pub(super) async fn set_delivery(&self, event: Uuid, status: &str, attempts: i32) {
        self.run(
            "UPDATE app_event_deliveries SET status = $2, attempts = $3 WHERE event_id = $1",
            vec![event.into(), status.into(), attempts.into()],
        )
        .await;
    }

    /// Record attempt `n` of `event`'s delivery, started `ago` seconds ago:
    /// finished with `outcome` and `error`, or still open when `outcome` is
    /// `None`.
    pub(super) async fn attempt(
        &self,
        event: Uuid,
        n: i32,
        outcome: Option<&str>,
        error: Option<&str>,
        ago: i64,
    ) {
        let delivery = self.delivery(event).await;
        self.run(
            "INSERT INTO app_event_delivery_attempts \
               (id, delivery_id, attempt, started_at, finished_at, outcome, error) \
             VALUES ($1, $2, $3, now() - make_interval(secs => $4), \
                     CASE WHEN $5::text IS NULL THEN NULL ELSE now() - make_interval(secs => $4) END, \
                     $5, $6)",
            vec![
                Uuid::new_v4().into(),
                delivery.id.into(),
                n.into(),
                (ago as f64).into(),
                outcome.map(str::to_string).into(),
                error.map(str::to_string).into(),
            ],
        )
        .await;
    }

    /// `GET path` as the guest.
    pub(super) async fn get(&self, path: &str) -> (StatusCode, Value) {
        call(Method::GET, path, None).await
    }

    /// `method path` with a JSON `body`, as the guest.
    pub(super) async fn send(
        &self,
        method: Method,
        path: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        call(method, path, Some(body)).await
    }

    /// The audit rows of `action` in this org: their metadata.
    pub(super) async fn audits(&self, action: &str) -> Vec<Value> {
        self.db()
            .query_all_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT metadata FROM audit_events WHERE org_id = $1 AND action = $2",
                [self.t.org_id.into(), action.into()],
            ))
            .await
            .expect("read the audit rows")
            .into_iter()
            .map(|row| row.try_get::<Value>("", "metadata").expect("metadata"))
            .collect()
    }
}

/// `method uri` (below `/api`), with `body` as JSON, through the router
/// `public_routes()` mounts, as the guest: its status and its JSON body
/// (`null` when it has none).
pub(super) async fn call(method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let request = Request::builder().method(method).uri(uri);
    let request = match body {
        Some(body) => request
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string())),
        None => request.body(Body::empty()),
    }
    .expect("request");
    let response = public_routes()
        .with_state(bare_app_state())
        .oneshot(request)
        .await
        .expect("oneshot");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

pub(super) async fn run_sql(db: &DatabaseConnection, sql: &str, args: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        args,
    ))
    .await
    .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

/// A timestamp the routes answered with.
pub(super) fn at(value: &Value) -> DateTime<Utc> {
    let text = value
        .as_str()
        .unwrap_or_else(|| panic!("a timestamp: {value}"));
    DateTime::parse_from_rfc3339(text)
        .unwrap_or_else(|e| panic!("{text}: {e}"))
        .with_timezone(&Utc)
}

/// A plain member of the org: neither its officer nor Oxy staff.
pub(super) async fn demote_guest(t: &Tenant) {
    run_sql(
        &t.db,
        "UPDATE org_members SET role = 'member' WHERE org_id = $1 AND user_id = $2",
        vec![t.org_id.into(), t.guest_id.into()],
    )
    .await;
}

/// Publish `functions` (and the manifest keys in `extra`) as `slug` in
/// `workspace`, promoted; the app's id.
async fn publish_app(
    t: &Tenant,
    slug: &str,
    workspace: Uuid,
    functions: &[FunctionSpec],
    extra: Value,
) -> Uuid {
    publish(PublishInput {
        org_ref: Some(OrgRef::Id(t.org_id)),
        app_slug: slug.to_string(),
        project_id: workspace,
        branch: None,
        build_id: format!("{slug}-1"),
        name: None,
        promote: true,
        tarball: crate::sandbox_publish::bundle(slug, functions, extra, &[]),
        manifest: None,
        source_repo: None,
        commit_sha: None,
        published_by: Some(t.guest_id),
        published_by_email: Some(LOCAL_GUEST_EMAIL.to_string()),
        publisher: Some(oxy_server_authz::Caller::without_credential(
            t.guest_id,
            LOCAL_GUEST_EMAIL,
        )),
        machine_app_id: None,
        published_via: None,
        semantic_revision_id: None,
        confirm_schedule_removals: Vec::new(),
    })
    .await
    .unwrap_or_else(|e| panic!("publish {slug}: {e}"))
    .app_id
}
