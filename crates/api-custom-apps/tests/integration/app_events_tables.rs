//! The platform tables behind app events: an event, its per-listener
//! deliveries and their attempts.
//!
//! The guarantees are the database's — one delivery per (event, listener
//! function), and a delete of an event taking its deliveries and attempts
//! with it — so they are tested against Postgres.

use chrono::Utc;
use entity::{apps, organizations, workspaces};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    Statement,
};
use uuid::Uuid;

use crate::common::test_db;

async fn seed_app(db: &DatabaseConnection) -> (Uuid, Uuid, Uuid) {
    let org_id = Uuid::new_v4();
    organizations::ActiveModel {
        id: Set(org_id),
        name: Set("App Events Org".into()),
        slug: Set(format!("app-events-{org_id}")),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed org");
    let workspace_id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: Set(workspace_id),
        name: Set("App Events Workspace".into()),
        org_id: Set(Some(org_id)),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed workspace");
    let app_id = Uuid::new_v4();
    apps::ActiveModel {
        id: Set(app_id),
        slug: Set(format!("events-{app_id}")),
        name: Set("Events".into()),
        org_id: Set(org_id),
        project_id: Set(workspace_id),
        branch: Set("main".into()),
        source_repo: Set("app-events/test".into()),
        status: Set("active".into()),
        source_type: Set("s3".into()),
        source_config: Set(serde_json::json!({})),
        published_at: Set(Some(Utc::now().into())),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed app");
    (org_id, workspace_id, app_id)
}

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .await
    .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

async fn count(db: &DatabaseConnection, table: &str) -> i64 {
    let row = db
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            format!("SELECT count(*) AS n FROM {table}"),
        ))
        .await
        .expect("count")
        .expect("a row");
    row.try_get::<i64>("", "n").expect("n")
}

#[tokio::test]
async fn app_events_tables() {
    let db = test_db().await;
    let (org_id, workspace_id, app_id) = seed_app(&db).await;

    let event_id = Uuid::new_v4();
    exec(
        &db,
        "INSERT INTO app_events (id, org_id, workspace_id, environment, source_app_id, type, \
         key, data_bytes, writer_role, root_id, depth, delay_seconds, published_at) \
         VALUES ($1, $2, $3, 'production', $4, 'order.placed', 'o-1', 2, 'app_shop_rw', $1, 0, 0, \
         now())",
        vec![
            event_id.into(),
            org_id.into(),
            workspace_id.into(),
            app_id.into(),
        ],
    )
    .await;

    let insert_delivery = |function: &'static str| {
        let id = Uuid::new_v4();
        (
            id,
            "INSERT INTO app_event_deliveries (id, event_id, app_id, function_name, \
             environment, status, attempts) VALUES ($1, $2, $3, $4, 'production', 'pending', 0)",
            vec![
                id.into(),
                event_id.into(),
                app_id.into(),
                function.to_string().into(),
            ],
        )
    };
    let (first, sql, values) = insert_delivery("on-order");
    exec(&db, sql, values).await;
    let (second, sql, values) = insert_delivery("on-order-audit");
    exec(&db, sql, values).await;

    // A third delivery for the same (event, app, function) is a duplicate.
    let (_, sql, values) = insert_delivery("on-order");
    let err = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            values,
        ))
        .await
        .expect_err("a duplicate delivery is refused");
    assert!(
        err.to_string().contains("duplicate key"),
        "expected a unique violation, got: {err}"
    );
    assert_eq!(count(&db, "app_event_deliveries").await, 2);

    for delivery in [first, second] {
        exec(
            &db,
            "INSERT INTO app_event_delivery_attempts (id, delivery_id, attempt, started_at) \
             VALUES ($1, $2, 1, now())",
            vec![Uuid::new_v4().into(), delivery.into()],
        )
        .await;
    }
    assert_eq!(count(&db, "app_event_delivery_attempts").await, 2);

    // Deleting the event takes its deliveries and their attempts.
    exec(
        &db,
        "DELETE FROM app_events WHERE id = $1",
        vec![event_id.into()],
    )
    .await;
    assert_eq!(count(&db, "app_event_deliveries").await, 0);
    assert_eq!(count(&db, "app_event_delivery_attempts").await, 0);
}

/// The entities read the rows the migration writes.
#[tokio::test]
async fn app_events_entities_read_the_tables() {
    use entity::{app_event_deliveries, app_events};
    use sea_orm::EntityTrait;

    let db = test_db().await;
    let (org_id, workspace_id, app_id) = seed_app(&db).await;
    let event_id = Uuid::new_v4();
    exec(
        &db,
        "INSERT INTO app_events (id, org_id, workspace_id, environment, source_app_id, type, \
         key, data_bytes, writer_role, root_id, depth, published_at) \
         VALUES ($1, $2, $3, 'staging', $4, 'order.placed', 'o-1', 8, 'app_shop_rw', $1, 0, now())",
        vec![
            event_id.into(),
            org_id.into(),
            workspace_id.into(),
            app_id.into(),
        ],
    )
    .await;
    let event = app_events::Entity::find_by_id(event_id)
        .one(&db)
        .await
        .expect("read event")
        .expect("event row");
    assert_eq!(event.r#type, "order.placed");
    assert_eq!(event.delay_seconds, 0, "delay defaults to none");
    // The body's size and where it is, never the body.
    assert_eq!(event.data_bytes, 8);
    assert_eq!(event.writer_role, "app_shop_rw");

    let delivery_id = Uuid::new_v4();
    exec(
        &db,
        "INSERT INTO app_event_deliveries (id, event_id, app_id, function_name, environment, status) \
         VALUES ($1, $2, $3, 'on-order', 'staging', 'pending')",
        vec![delivery_id.into(), event_id.into(), app_id.into()],
    )
    .await;
    let delivery = app_event_deliveries::Entity::find_by_id(delivery_id)
        .one(&db)
        .await
        .expect("read delivery")
        .expect("delivery row");
    assert_eq!(
        (delivery.status.as_str(), delivery.attempts),
        ("pending", 0)
    );

    // A status outside the documented set is refused.
    let err = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE app_event_deliveries SET status = 'bogus' WHERE id = $1",
            vec![delivery_id.into()],
        ))
        .await
        .expect_err("an unknown status is refused");
    assert!(err.to_string().contains("status_check"), "{err}");
}
