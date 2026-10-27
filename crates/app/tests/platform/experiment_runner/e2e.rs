//! A whole run on DuckDB. The reader here stands in for `RunnerUnitDays`
//! (the production reader over the metric-tree runner). The view is
//! time-invariant, so the days a unit reads do not depend on when the check
//! runs. Abort is covered in `actuation.rs`, not here.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use agentic_core::delegation::TaskSpec;
use async_trait::async_trait;
use chrono::{Days, Duration, NaiveDate};
use entity::automation_definitions;
use oxy_airlayer_compat::engine::query::{FilterOperator, QueryFilter, QueryRequest};
use oxy_airlayer_compat::{DatasourceDialectMap, SemanticEngine, SemanticLayer};
use oxy_app::server::experiment::check::reader::{Axis, Membership, UnitDays, unit_axis};
use oxy_app::server::experiment::check::{Readers, run_check};
use oxy_app::server::experiment::panel::GroupMembership;
use oxy_app::server::experiment::status::run_status;
use oxy_app::server::experiment::store;
use oxy_experiment::timeline::{day_start_utc, last_post_window_end_date};
use sea_orm::{
    ColumnTrait, DatabaseBackend, EntityTrait, FromQueryResult, QueryFilter as _, Statement,
};

use super::check::Scan;
use super::fixture::*;

const STORES: &[&str] = &[
    "bondi",
    "newtown",
    "manly",
    "surry_hills",
    "glebe",
    "paddington",
];
const PROMO: &str = "store_days.promo_active";

const VIEW: &str = r#"
name: store_days
datasource: local
sql: |
  SELECT c.store, c.day,
    CASE WHEN (SELECT e.state FROM promo_events e WHERE e.store = c.store
               ORDER BY e.seq DESC LIMIT 1) = 'on' THEN 1 ELSE 0 END AS promo_active,
    100.0 AS beverages_revenue
  FROM store_calendar c
entities:
  - name: store
    type: primary
    key: store
dimensions:
  - name: store
    type: string
    expr: store
  - name: day
    type: date
    expr: day
measures:
  - name: promo_active
    type: sum
    expr: promo_active
  - name: beverages_revenue
    type: sum
    expr: beverages_revenue
"#;

/// One in-memory warehouse: the flag table the lever writes, and a calendar.
struct Warehouse {
    conn: duckdb::Connection,
    layer: SemanticLayer,
}

impl Warehouse {
    fn new() -> Self {
        let conn = duckdb::Connection::open_in_memory().expect("duckdb");
        let stores = STORES
            .iter()
            .map(|s| format!("('{s}')"))
            .collect::<Vec<_>>()
            .join(", ");
        conn.execute_batch(&format!(
            "CREATE SEQUENCE promo_seq;
             CREATE TABLE promo_events (seq BIGINT DEFAULT nextval('promo_seq'), store VARCHAR, state VARCHAR);
             CREATE TABLE store_calendar AS
               SELECT s.store, CAST(g.d AS DATE) AS day
               FROM (VALUES {stores}) s(store),
                    generate_series(DATE '2026-01-01', DATE '2026-07-31', INTERVAL 1 DAY) g(d);"
        ))
        .expect("seed the warehouse");
        let view = oxy_airlayer_compat::parse_view_yaml(VIEW).expect("the view parses");
        Self {
            conn,
            layer: SemanticLayer::new(vec![view], None),
        }
    }

    fn workspace(&self) -> Levers {
        Levers {
            conn: Mutex::new(self.conn.try_clone().expect("clone")),
        }
    }

    fn days(&self) -> Reader {
        let dialects = DatasourceDialectMap::with_default(oxy_airlayer_compat::Dialect::DuckDB);
        Reader {
            conn: Mutex::new(self.conn.try_clone().expect("clone")),
            engine: SemanticEngine::from_semantic_layer(self.layer.clone(), dialects)
                .expect("engine"),
            layer: self.layer.clone(),
        }
    }
}

/// What the worker would give the lever: a `local` database on the warehouse.
struct Levers {
    conn: Mutex<duckdb::Connection>,
}

#[async_trait]
impl agentic_automation::WorkspaceContext for Levers {
    fn workspace_path(&self) -> Option<&Path> {
        None
    }
    fn database_configs(&self) -> Vec<oxy_airlayer_compat::DatabaseConfig> {
        vec![]
    }
    async fn get_connector(
        &self,
        name: &str,
    ) -> Result<Arc<dyn agentic_connector::DatabaseConnector>, String> {
        match name {
            "local" => Ok(Arc::new(agentic_connector::DuckDbConnector::new(
                self.conn
                    .lock()
                    .expect("warehouse lock")
                    .try_clone()
                    .map_err(|e| e.to_string())?,
            ))),
            other => Err(format!("no connector {other}")),
        }
    }
    async fn get_integration(
        &self,
        name: &str,
    ) -> Result<agentic_automation::workspace::IntegrationConfig, String> {
        Err(format!("no integration {name}"))
    }
    async fn list_automation_files(&self) -> Result<Vec<PathBuf>, String> {
        Ok(vec![])
    }
    async fn resolve_automation_yaml(
        &self,
        r: &str,
    ) -> Result<String, agentic_pipeline::WorkspaceReadError> {
        let _ = r;
        Err("the lever body is passed inline, never resolved by ref".into())
    }
}

/// The read-back measure, read through the semantic layer on the warehouse.
struct Reader {
    conn: Mutex<duckdb::Connection>,
    engine: SemanticEngine,
    layer: SemanticLayer,
}

fn filter(member: &str, op: FilterOperator, value: &str) -> QueryFilter {
    QueryFilter {
        member: Some(member.into()),
        operator: Some(op),
        values: vec![value.into()],
        and: None,
        or: None,
    }
}

#[async_trait]
impl UnitDays for Reader {
    async fn values(
        &self,
        measure: &str,
        unit: &str,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<BTreeMap<NaiveDate, f64>, String> {
        let axis = unit_axis(
            &self.layer,
            &Axis {
                for_each: "store".into(),
                time_dimension: None,
            },
            measure,
        )?;
        let request = QueryRequest {
            measures: vec![measure.to_string()],
            dimensions: vec![axis.time_member.clone()],
            filters: vec![
                filter(&axis.unit_member, FilterOperator::Equals, unit),
                filter(
                    &axis.time_member,
                    FilterOperator::AfterOrOnDate,
                    &from.to_string(),
                ),
                filter(
                    &axis.time_member,
                    FilterOperator::BeforeOrOnDate,
                    &to.to_string(),
                ),
            ],
            limit: Some(oxy_airlayer_compat::engine::UNBOUNDED_QUERY_LIMIT),
            ..QueryRequest::new()
        };
        let compiled = self
            .engine
            .compile_query(&request)
            .map_err(|e| e.to_string())?;
        let sql = format!(
            "SELECT CAST(q.{day} AS VARCHAR), CAST(q.{value} AS DOUBLE) FROM ({inner}) q",
            day = axis.time_member.replace('.', "__"),
            value = measure.replace('.', "__"),
            inner = compiled.sql,
        );
        let conn = self.conn.lock().expect("warehouse lock");
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(duckdb::params_from_iter(compiled.params.iter()), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
            })
            .map_err(|e| e.to_string())?;
        rows.map(|r| {
            let (day, v) = r.map_err(|e| e.to_string())?;
            Ok((
                day[..10].parse::<NaiveDate>().map_err(|e| e.to_string())?,
                v,
            ))
        })
        .collect()
    }

    async fn units(&self, _measure: &str, _since_days: u32) -> Result<Vec<String>, String> {
        Ok(STORES.iter().map(|s| s.to_string()).collect())
    }
}

/// The design has no `assign_by`, so a check never asks.
#[async_trait]
impl Membership for Reader {
    async fn groups(&self, _entity: &str, dimension: &str) -> Result<GroupMembership, String> {
        Err(format!(
            "this design groups nothing, yet `{dimension}` was asked for"
        ))
    }
}

/// Run every queued lever job the way the worker would, minus the network.
async fn drain(fx: &Fx, levers: &Levers) -> usize {
    #[derive(FromQueryResult)]
    struct Queued {
        run_id: String,
        spec: serde_json::Value,
    }
    let queued = Queued::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT q.run_id, q.spec FROM agentic_task_queue q JOIN agentic_runs r ON r.id = q.run_id \
         WHERE r.workspace_id = $1 AND r.metadata->>'trigger' = 'experiment' \
           AND r.task_status = 'running' ORDER BY r.created_at",
        [fx.ws.into()],
    ))
    .all(&fx.db)
    .await
    .expect("read the queue");
    for job in &queued {
        let spec: TaskSpec = serde_json::from_value(job.spec.clone()).expect("a task spec");
        let TaskSpec::Automation {
            workflow_ref,
            variables,
            ..
        } = spec
        else {
            panic!("a lever job is an automation");
        };
        let lever = automation_definitions::Entity::find()
            .filter(automation_definitions::Column::FilePath.eq(workflow_ref))
            .one(&fx.db)
            .await
            .expect("read the lever")
            .expect("the lever exists");
        let config: agentic_automation::AutomationConfig =
            serde_json::from_value(lever.definition).expect("the lever parses");
        agentic_pipeline::automation_run::run_inline_automation_with(
            levers, config, variables, None,
        )
        .await
        .expect("the lever runs");
        fx.exec(
            "UPDATE agentic_runs SET task_status = 'done' WHERE id = $1",
            [job.run_id.clone().into()],
        )
        .await;
    }
    queued.len()
}

async fn check_at(fx: &Fx, days: &Reader, at: chrono::DateTime<chrono::Utc>) {
    let run = fx.run().await;
    run_check(
        &fx.db,
        &run,
        &Readers {
            days,
            anomalies: &Scan(None),
            groups: days,
        },
        at,
    )
    .await
    .expect("check completes");
}

fn sydney_start(day: NaiveDate) -> chrono::DateTime<chrono::Utc> {
    day_start_utc(day, chrono_tz::Australia::Sydney)
}

#[tokio::test]
async fn a_run_flips_reads_back_closes_and_turns_off_with_no_network() {
    let fx = Fx::seed(DESIGN).await;
    let warehouse = Warehouse::new();
    let (levers, days) = (warehouse.workspace(), warehouse.days());
    fx.register().await;

    switch_and_confirm(&fx, &levers, &days, "2026-03-02", &["bondi", "newtown"]).await;
    switch_and_confirm(&fx, &levers, &days, "2026-03-16", &["manly", "surry_hills"]).await;

    let end = last_post_window_end_date(&fx.design().await).expect("an end");
    close_and_estimate(&fx, &levers, &days, end).await;
    report(&fx, &levers, end).await;

    let status = run_status(&fx.db, fx.ws, NAME, sydney_start(end + Days::new(1)))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        status.lever_revisions,
        vec![fx.head().await],
        "one lever revision for the whole run"
    );
    let compliance = status.run.unwrap().compliance.expect("decided");
    assert_eq!(
        (compliance.c_treated, compliance.c_control_leak),
        (1.0, 0.0)
    );
    assert!(!compliance.below_floor);
}

async fn switch_and_confirm(fx: &Fx, levers: &Levers, days: &Reader, day: &str, units: &[&str]) {
    fx.tick(switch_at(day)).await;
    assert_eq!(
        drain(fx, levers).await,
        units.len(),
        "one lever run per unit on {day}"
    );
    check_at(fx, days, switch_at(day) + Duration::hours(20)).await;
    for unit in units {
        let flip = fx.flip(unit, "on").await;
        assert_eq!(
            (flip.status.as_str(), flip.confirmed_on),
            ("confirmed", Some(d(day).and_hms_opt(0, 0, 0).unwrap())),
            "{unit}"
        );
    }
    let obs = store::observations(&fx.run().await);
    assert!(
        obs.control_leaks.is_empty(),
        "no control saw the promo: {:?}",
        obs.control_leaks
    );
}

async fn close_and_estimate(fx: &Fx, levers: &Levers, days: &Reader, end: NaiveDate) {
    fx.tick(sydney_start(end)).await;
    assert_eq!(
        fx.run().await.state,
        "closing",
        "closes ON the exclusive end"
    );
    assert_eq!(
        drain(fx, levers).await,
        4,
        "on_close: off — every treated unit, on entering closing, before any estimate"
    );
    let flag = days.values(PROMO, "bondi", end, end).await.unwrap();
    assert_eq!(
        flag.into_values().collect::<Vec<_>>(),
        vec![0.0],
        "the measure reads the lever off"
    );
    check_at(fx, days, sydney_start(end) + Duration::hours(6)).await;
    assert_eq!(store::observations(&fx.run().await).coverage, Some(1.0));
    fx.tick(sydney_start(end) + Duration::hours(7)).await;
    fx.tick(sydney_start(end) + Duration::hours(8)).await;
    assert_eq!(
        fx.queued_kind("experiment_estimate").await,
        1,
        "enqueued once"
    );
}

async fn report(fx: &Fx, levers: &Levers, end: NaiveDate) {
    let estimate = fx.run().await.estimate_task_id.expect("the estimate job");
    // PR 5's executor completes only after writing the result row.
    fx.exec(
        "UPDATE agentic_runs SET task_status = 'done' WHERE id = $1",
        [estimate.to_string().into()],
    )
    .await;
    fx.tick(sydney_start(end + Days::new(1))).await;
    assert_eq!(fx.run().await.state, "reported");
    assert_eq!(drain(fx, levers).await, 0, "reporting sends nothing new");
    fx.tick(sydney_start(end + Days::new(2))).await;
    assert_eq!(drain(fx, levers).await, 0, "nothing is sent twice");
}
