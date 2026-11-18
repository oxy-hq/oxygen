//! Staging's and the sandboxes' events reaching their listeners: relayed from
//! the branch, held by the environment rules (Task 9, unchanged), and
//! delivered by the production executor, which reads each body from the
//! branch.

use agentic_core::delegation::{TaskOutcome, TaskSpec};
use entity::app_event_delivery_attempts as attempts;
use entity::{app_environments, app_event_deliveries as deliveries};
use oxy_api_custom_apps::app_events::retention::retire_environment_events;
use oxy_oltp::events::{self as outbox, OutboxKey, OutboxTarget};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, EntityTrait, QueryFilter, QueryOrder, Statement,
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{BROKEN_JS, Fx, OK_JS, with_fx};
use crate::env_schedules::run_spec;

impl Fx {
    /// `app`'s one delivery, if it has one.
    async fn delivery_to(&self, app: Uuid) -> Option<deliveries::Model> {
        let mut found: Vec<_> = self
            .deliveries()
            .await
            .into_iter()
            .filter(|d| d.app_id == app)
            .collect();
        assert!(found.len() <= 1, "{found:?}");
        found.pop()
    }

    /// The delivery's queued task, by its `run_id`: what a worker claims.
    async fn task(&self, delivery: &deliveries::Model) -> Value {
        let run_id = delivery.run_id.clone().expect("a pending delivery's run");
        let row = self
            .t
            .db
            .query_one_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT spec FROM agentic_task_queue WHERE task_id = $1",
                [run_id.into()],
            ))
            .await
            .expect("read the queue")
            .expect("the delivery's task");
        row.try_get("", "spec").expect("spec")
    }

    /// Run the delivery's task once through the production executor.
    async fn run(&self, delivery: &deliveries::Model) -> TaskOutcome {
        let spec: TaskSpec =
            serde_json::from_value(self.task(delivery).await).expect("a task spec");
        run_spec(&self.t.db, spec).await
    }

    async fn reload(&self, delivery: &deliveries::Model) -> deliveries::Model {
        deliveries::Entity::find_by_id(delivery.id)
            .one(&self.t.db)
            .await
            .expect("read the delivery")
            .expect("the delivery is still there")
    }
}

#[tokio::test]
async fn a_staging_publish_reaches_a_staging_listener_when_the_switch_is_on() {
    with_fx(|fx| async move {
        let (listener, _) = fx.listener("br-l", OK_JS, json!({})).await;
        fx.switch_on(listener, "staging").await;
        let id = fx.publish("staging", "statement:1").await;

        let stats = fx.relay("staging").await;
        assert_eq!((stats.relayed, stats.deliveries, stats.rejected), (1, 1, 0));
        let events = fx.events().await;
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].id, id);
        assert_eq!(events[0].environment, "staging");
        assert_eq!(events[0].relay_error, None);
        let delivery = fx
            .delivery_to(listener)
            .await
            .expect("the listener's delivery");
        assert_eq!(
            (delivery.environment.as_str(), delivery.status.as_str()),
            ("staging", "pending")
        );

        // The run reads the body from the branch, by `(writer_role, id)`.
        let outcome = fx.run(&delivery).await;
        let TaskOutcome::Done { answer, .. } = &outcome else {
            panic!("delivered: {outcome:?}");
        };
        assert_eq!(answer, "delivered (HTTP 200)");
        let acked = fx.reload(&delivery).await;
        assert_eq!((acked.status.as_str(), acked.attempts), ("acked", 1));
    })
    .await;
}

#[tokio::test]
async fn with_the_switch_off_the_delivery_is_held() {
    with_fx(|fx| async move {
        let (listener, _) = fx.listener("br-l", OK_JS, json!({})).await;
        fx.publish("staging", "statement:1").await;

        assert_eq!(fx.relay("staging").await.deliveries, 1);
        let delivery = fx
            .delivery_to(listener)
            .await
            .expect("the listener's delivery");
        assert_eq!(
            (
                delivery.environment.as_str(),
                delivery.status.as_str(),
                delivery.run_id
            ),
            ("staging", "held", None),
            "held, and nothing queued"
        );
    })
    .await;
}

/// `dev-x` reaches the listeners' `dev-x` only: an app whose subscription
/// outlived its `dev-x` holds the event, and an app with no `dev-x` gets
/// nothing.
#[tokio::test]
async fn dev_x_reaches_dev_x_and_holds_for_an_app_without_dev_x() {
    with_fx(|fx| async move {
        fx.publisher_sandbox("x").await;
        let (a, a_slug) = fx.listener("br-a", OK_JS, json!({})).await;
        let (b, b_slug) = fx.listener("br-b", OK_JS, json!({})).await;
        let (c, _) = fx.listener("br-c", OK_JS, json!({})).await;
        for (app, slug) in [(a, &a_slug), (b, &b_slug)] {
            let on = fx.function(OK_JS, json!({}));
            fx.sandbox(app, slug, "x", &[on], json!({})).await;
            fx.switch_on(app, "dev-x").await;
        }
        app_environments::Entity::delete_by_id((b, "dev-x".to_string()))
            .exec(&fx.t.db)
            .await
            .expect("drop b's dev-x");
        let id = fx.publish("dev-x", "statement:1").await;

        let stats = fx.relay("dev-x").await;
        assert_eq!((stats.relayed, stats.deliveries, stats.rejected), (1, 2, 0));
        let events = fx.events().await;
        assert_eq!((events.len(), events[0].id), (1, id));
        assert_eq!(
            events[0].environment, "dev-x",
            "a sandbox hosted on the branch stands"
        );
        let pending = fx.delivery_to(a).await.expect("a's delivery");
        assert_eq!(
            (pending.environment.as_str(), pending.status.as_str()),
            ("dev-x", "pending")
        );
        assert_eq!(
            fx.task(&pending).await["payload"]["environment"],
            json!("dev-x")
        );
        let held = fx.delivery_to(b).await.expect("b's delivery");
        assert_eq!(
            (held.environment.as_str(), held.status.as_str()),
            ("dev-x", "held")
        );
        assert!(fx.delivery_to(c).await.is_none(), "no dev-x, no delivery");
    })
    .await;
}

#[tokio::test]
async fn a_staging_event_never_reaches_production() {
    with_fx(|fx| async move {
        // Listens in production and in staging.
        let (listener, _) = fx.listener("br-l", OK_JS, json!({})).await;
        fx.switch_on(listener, "staging").await;
        fx.publish("staging", "statement:1").await;

        assert_eq!(
            fx.relay("production").await,
            Default::default(),
            "production's relay never reads the branch"
        );
        fx.relay("staging").await;
        let deliveries = fx.deliveries().await;
        assert_eq!(deliveries.len(), 1, "{deliveries:?}");
        assert_eq!(deliveries[0].environment, "staging");
        let task = fx.task(&deliveries[0]).await;
        assert_eq!(task["payload"]["environment"], json!("staging"));
        assert_eq!(task["payload"]["input"]["environment"], json!("staging"));
        assert!(fx.events().await.iter().all(|e| e.environment == "staging"));
    })
    .await;
}

/// The controller adds the `dead` route's check after the merge (Task 15).
#[tokio::test]
async fn a_sandbox_dead_letter_is_recorded_dead_with_every_attempt() {
    with_fx(|fx| async move {
        fx.publisher_sandbox("x").await;
        let (listener, slug) = fx.listener("br-l", OK_JS, json!({})).await;
        let broken = fx.function(BROKEN_JS, json!({ "retries": { "maxAttempts": 2 } }));
        fx.sandbox(listener, &slug, "x", &[broken], json!({})).await;
        fx.switch_on(listener, "dev-x").await;
        fx.publish("dev-x", "statement:1").await;
        fx.relay("dev-x").await;
        let delivery = fx
            .delivery_to(listener)
            .await
            .expect("the listener's delivery");

        for attempt in 1..=2 {
            let outcome = fx.run(&delivery).await;
            assert!(
                matches!(outcome, TaskOutcome::Failed(_)),
                "attempt {attempt}: {outcome:?}"
            );
        }
        let dead = fx.reload(&delivery).await;
        assert_eq!(
            (
                dead.status.as_str(),
                dead.attempts,
                dead.environment.as_str()
            ),
            ("dead", 2, "dev-x")
        );
        let reason = dead.last_error.unwrap_or_default();
        assert!(reason.contains("the listener broke"), "{reason}");
        let tries: Vec<(i32, Option<String>)> = attempts::Entity::find()
            .filter(attempts::Column::DeliveryId.eq(delivery.id))
            .order_by_asc(attempts::Column::Attempt)
            .all(&fx.t.db)
            .await
            .expect("read the attempts")
            .into_iter()
            .map(|a| (a.attempt, a.outcome))
            .collect();
        let error = Some("error".to_string());
        assert_eq!(
            tries,
            [(1, error.clone()), (2, error)],
            "every attempt is recorded"
        );
    })
    .await;
}

/// A sandbox's teardown retires its events from the platform alone, so it
/// stands once the branch they were relayed from is gone; reading or deleting
/// a body there then finds nothing, and fails nothing.
#[tokio::test]
async fn a_sandbox_teardown_stands_once_its_store_is_gone() {
    with_fx(|fx| async move {
        fx.publisher_sandbox("x").await;
        let id = fx.publish("dev-x", "statement:1").await;
        fx.relay("dev-x").await;
        let writer_role = fx.events().await[0].writer_role.clone();
        fx.store.delete_branches().await;

        let retired = retire_environment_events(&fx.t.db, fx.publisher, "dev-x")
            .await
            .expect("the teardown's part stands");
        assert_eq!(retired, 1);
        assert!(fx.events().await.is_empty());
        let gone = OutboxTarget::Branch("dev-x".into());
        let body = outbox::read_body(&fx.t.db, fx.org(), &gone, &writer_role, id)
            .await
            .expect("no store, no body");
        assert_eq!(body, None);
        let key = OutboxKey { writer_role, id };
        let deleted = outbox::delete_bodies(&fx.t.db, fx.org(), &gone, &[key])
            .await
            .expect("nothing to delete");
        assert_eq!(deleted, 0);
    })
    .await;
}
