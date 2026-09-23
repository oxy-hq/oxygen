//! The executor's cursor reset: what it refuses, what it lets through, and
//! what it does NOT need in order to run.
//!
//! `reset_airway_schema` is destructive and airhouse-only — it drops the
//! destination tables, so it has to re-mint a credential to do it. A cursor
//! reset drops nothing, which buys it two properties this module pins:
//! it works against any destination kind, and it can refuse.
//!
//! The refusal is the part worth guarding. Keeping the tables while rewinding
//! the cursor is exactly right for a merge-keyed resource and a silent
//! data-corruption button for an append-only one, so an executor that clears
//! cursors without consulting the stored schema is worse than one that has no
//! cursor reset at all.
//!
//! Requires Docker (or `OXY_DATABASE_URL`); self-skips otherwise.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agentic_airway::extension::pipeline_lease::{self, LEASE_TTL_SECS, LeaseAcquisition};
use agentic_airway::reset::CursorScope;
use agentic_airway::{AirwayMigrator, AirwayPgStateStore};
use agentic_pipeline::executor::{PipelineTaskExecutor, ResetCursorsError};
use agentic_runtime::migration::RuntimeMigrator;
use airway::Schema;
use airway::schema::{Column, Table};
use airway::state::{PipelineState, ResourceState, StateStore};
use airway::types::{DataType, WriteDisposition};
use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

static TEST_DB_URL: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();
static TEST_CONTAINER: tokio::sync::OnceCell<
    Arc<testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>>,
> = tokio::sync::OnceCell::const_new();

async fn test_db() -> Option<DatabaseConnection> {
    let url = TEST_DB_URL
        .get_or_init(|| async {
            if let Ok(url) = std::env::var("OXY_DATABASE_URL") {
                return url;
            }
            use testcontainers::runners::AsyncRunner;
            use testcontainers::{ImageExt, ReuseDirective};
            use testcontainers_modules::postgres::Postgres;
            let container = TEST_CONTAINER
                .get_or_init(|| async {
                    Arc::new(
                        Postgres::default()
                            .with_tag("18-alpine")
                            .with_reuse(ReuseDirective::Always)
                            .start()
                            .await
                            .expect("start Postgres testcontainer — is Docker running?"),
                    )
                })
                .await;
            let port = container.get_host_port_ipv4(5432_u16).await.unwrap();
            format!("postgresql://postgres:postgres@127.0.0.1:{port}/postgres")
        })
        .await
        .clone();

    let mut db = None;
    for attempt in 0..10 {
        match sea_orm::Database::connect(&url).await {
            Ok(conn) => {
                db = Some(conn);
                break;
            }
            Err(e) if attempt < 9 => {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                eprintln!("test_db: attempt {attempt} failed: {e}");
            }
            Err(_) => return None,
        }
    }
    let db = db?;
    oxy_test_utils::migration::migrate_shared_test_db::<RuntimeMigrator>(&url, &db)
        .await
        .expect("shared migrations failed")
        .then::<AirwayMigrator>()
        .await
        .expect("airway migrations")
        .finish()
        .await;
    Some(db)
}

/// A workspace that serves one compiled `.airway.yml`.
///
/// No `workspace_id` field: `PlatformContext` is a blanket impl over
/// `ProjectContext + WorkflowWorkspaceContext`, so a fake cannot override it
/// and every test here runs as [`WORKSPACE_ID`] (the trait's nil default —
/// also what local mode uses). The state row is keyed by
/// `(workspace_id, pipeline_name)`, so isolation on the shared test database
/// comes from the per-test pipeline name instead.
struct FakeWorkspace {
    yaml: String,
}

/// What `PlatformContext::workspace_id` reports for the fake above.
const WORKSPACE_ID: Uuid = Uuid::nil();

#[async_trait]
impl agentic_pipeline::platform::ProjectContext for FakeWorkspace {
    async fn resolve_connector(
        &self,
        _db_name: &str,
    ) -> Option<agentic_connector::ConnectorConfig> {
        None
    }
    async fn resolve_model(
        &self,
        _model_ref: Option<&str>,
        _has_explicit_model: bool,
    ) -> Option<agentic_analytics::config::ResolvedModelInfo> {
        None
    }
    async fn resolve_secret(&self, _var_name: &str) -> Option<String> {
        None
    }
}

#[async_trait]
impl agentic_automation::WorkspaceContext for FakeWorkspace {
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
        Err(format!("fake workspace: connector '{name}' unavailable"))
    }
    async fn get_integration(
        &self,
        name: &str,
    ) -> Result<agentic_automation::workspace::IntegrationConfig, String> {
        Err(format!("fake workspace: integration '{name}' unavailable"))
    }
    async fn list_automation_files(&self) -> Result<Vec<PathBuf>, String> {
        Ok(vec![])
    }
    async fn resolve_automation_yaml(
        &self,
        _workflow_ref: &str,
    ) -> Result<String, agentic_pipeline::WorkspaceReadError> {
        Err("fake workspace: not available".into())
    }
    async fn resolve_pipeline_yaml(&self, _pipeline_ref: &str) -> Result<Option<String>, String> {
        Ok(Some(self.yaml.clone()))
    }
}

/// A `destination:` **reference**, deliberately — that is what users author,
/// and it is what `reset_airway_schema` has to resolve into a credentialed
/// airhouse connector before it can drop anything. A cursor reset drops
/// nothing, so it must never reach that resolution.
fn pipeline_yaml(name: &str) -> String {
    format!(
        r#"
name: {name}
source:
  kind: filesystem
  config:
    base_path: /tmp/airway-cursor-reset
    pattern: "*.jsonl"
    format: jsonl
    table_name: vendor_sales
destination:
  database: warehouse
  dataset_name: raw
resources:
  - vendor_sales
  - vendor_forecasting
"#
    )
}

/// `amazon_vc`'s shape: a merge-keyed resource beside an append-only one that
/// holds history the source will not serve twice.
fn amazon_vc_schema(name: &str) -> Schema {
    let mut schema = Schema::new(name);

    let mut sales = Table::new("vendor_sales");
    sales.write_disposition = WriteDisposition::Merge;
    let mut asin = Column::new("asin", DataType::Text);
    asin.primary_key = true;
    sales.columns.insert("asin".to_string(), asin);
    schema.tables.insert("vendor_sales".to_string(), sales);

    let mut forecasting = Table::new("vendor_forecasting");
    forecasting.write_disposition = WriteDisposition::Append;
    schema
        .tables
        .insert("vendor_forecasting".to_string(), forecasting);

    schema
}

fn state_with(cursors: &[(&str, &str)]) -> PipelineState {
    let mut state = PipelineState::default();
    for (resource, high_water) in cursors {
        state.resource_states.insert(
            (*resource).to_string(),
            ResourceState {
                incremental: None,
                custom: HashMap::from([(
                    "__connector_state".to_string(),
                    serde_json::json!({ "high_water": high_water }),
                )]),
            },
        );
    }
    state
}

fn high_water_of(state: &PipelineState, resource: &str) -> Option<String> {
    state
        .resource_states
        .get(resource)?
        .custom
        .get("__connector_state")?
        .get("high_water")?
        .as_str()
        .map(str::to_string)
}

/// An executor over a workspace serving `pipeline_yaml(name)`, with the
/// pipeline's state seeded as if both resources had run.
async fn seeded(db: &DatabaseConnection, name: &str) -> (PipelineTaskExecutor, AirwayPgStateStore) {
    let store = AirwayPgStateStore::new(Arc::new(db.clone()), WORKSPACE_ID, name);
    store
        .save(
            &state_with(&[
                ("vendor_sales", "2026-09-20"),
                ("vendor_forecasting", "2026-09-21"),
            ]),
            &amazon_vc_schema(name),
            0,
        )
        .await
        .expect("seed pipeline state");

    let platform: Arc<dyn agentic_pipeline::platform::PlatformContext> = Arc::new(FakeWorkspace {
        yaml: pipeline_yaml(name),
    });
    (PipelineTaskExecutor::bare(platform, db.clone()), store)
}

/// **THE regression.** `vendor_forecasting` appends with no merge key, so
/// re-pulling it lands every row again. An executor that clears the cursors
/// without consulting the stored schema hands an operator a button that
/// silently duplicates — and duplicates in an append table are the failure
/// nobody notices until a total is wrong months later.
#[tokio::test(flavor = "multi_thread")]
async fn cursor_reset_refuses_when_a_table_in_scope_would_duplicate() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let name = format!("amazon_vc_{}", Uuid::new_v4().simple());
    let (executor, store) = seeded(&db, &name).await;

    let err = executor
        .reset_airway_cursors(
            "pipelines/amazon_vc.airway.yml",
            &CursorScope::AllResources,
            false,
        )
        .await
        .expect_err("resetting every cursor must refuse: one resource appends");

    let ResetCursorsError::Refused(refusals) = &err else {
        panic!("expected a Refused, got {err:?}");
    };
    assert!(
        refusals.iter().all(|r| r.kind() == "would_duplicate"),
        "every table here is claimed, so nothing is of unknown ownership: {refusals:?}"
    );
    let rendered = refusals
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(
        rendered.contains("vendor_forecasting"),
        "the refusal must name the resource it is about: {rendered}"
    );
    assert!(
        rendered.contains("duplicate rows"),
        "…and say what would happen: {rendered}"
    );

    // And it must be a refusal, not a partial application.
    let after = store.load().await.unwrap().state;
    assert_eq!(
        high_water_of(&after, "vendor_sales").as_deref(),
        Some("2026-09-20"),
        "a refused reset must not have cleared anything"
    );
}

/// Per-resource granularity is what makes the refusal usable rather than a
/// dead end: the append-only sibling blocks a pipeline-wide reset and blocks
/// nothing else. This is exactly the BMG backfill that was not run.
#[tokio::test(flavor = "multi_thread")]
async fn a_scoped_reset_proceeds_past_an_append_only_sibling() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let name = format!("amazon_vc_{}", Uuid::new_v4().simple());
    let (executor, store) = seeded(&db, &name).await;

    let cleared = executor
        .reset_airway_cursors(
            "pipelines/amazon_vc.airway.yml",
            &CursorScope::Resources(vec!["vendor_sales".into()]),
            false,
        )
        .await
        .expect("a merge-keyed resource converges on a re-pull, so this must be allowed");
    assert_eq!(cleared.cleared, vec!["vendor_sales".to_string()]);

    let after = store.load().await.unwrap();
    assert_eq!(high_water_of(&after.state, "vendor_sales"), None);
    assert_eq!(
        high_water_of(&after.state, "vendor_forecasting").as_deref(),
        Some("2026-09-21"),
        "the append-only resource keeps its cursor"
    );
    assert!(
        after.schema.is_some(),
        "and the stored schema — hence the destination tables — survives"
    );
}

/// The idempotent retry of a request that just worked must work again. Once
/// `vendor_sales`'s cursor is gone it holds nothing, and attributing tables
/// against held cursors alone left `vendor_sales` unclaimed — widening the
/// second call to the whole schema, where `vendor_forecasting` refused it. A
/// guard that refuses its own successful request teaches `force`.
#[tokio::test(flavor = "multi_thread")]
async fn repeating_a_scoped_reset_that_succeeded_succeeds_again() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let name = format!("amazon_vc_{}", Uuid::new_v4().simple());
    let (executor, store) = seeded(&db, &name).await;
    let scope = CursorScope::Resources(vec!["vendor_sales".into()]);

    let first = executor
        .reset_airway_cursors("pipelines/amazon_vc.airway.yml", &scope, false)
        .await
        .expect("first scoped reset");
    assert_eq!(first.cleared, vec!["vendor_sales".to_string()]);

    let second = executor
        .reset_airway_cursors("pipelines/amazon_vc.airway.yml", &scope, false)
        .await
        .expect("repeating a scoped reset that just succeeded must not be refused");
    assert!(second.cleared.is_empty(), "{second:?}");
    assert_eq!(second.not_held, vec!["vendor_sales".to_string()]);

    assert_eq!(
        high_water_of(&store.load().await.unwrap().state, "vendor_forecasting").as_deref(),
        Some("2026-09-21"),
        "the sibling the caller never named keeps its cursor"
    );
}

/// A reset landing mid-run bumps the state row's version, so the run's
/// best-effort `save` fails wholesale and drops *every* resource's advanced
/// cursor — `vendor_forecasting`'s included, though it is outside the scope —
/// and the next run re-pulls that window into an append-only table. The
/// judgement allows this very request, so only the lease can stop it.
///
/// And `force` must not get past it: `force` overrides a judgement, and a
/// held lease is not one.
#[tokio::test(flavor = "multi_thread")]
async fn a_cursor_reset_refuses_while_a_run_holds_the_lease() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let name = format!("amazon_vc_{}", Uuid::new_v4().simple());
    let (executor, store) = seeded(&db, &name).await;
    let run_id = format!("run-{}", Uuid::new_v4());
    assert_eq!(
        pipeline_lease::try_acquire(&db, WORKSPACE_ID, &name, &run_id, LEASE_TTL_SECS)
            .await
            .unwrap(),
        LeaseAcquisition::Acquired
    );

    for force in [false, true] {
        let err = executor
            .reset_airway_cursors(
                "pipelines/amazon_vc.airway.yml",
                &CursorScope::Resources(vec!["vendor_sales".into()]),
                force,
            )
            .await
            .expect_err("a reset must not land while a run holds the lease");
        assert!(
            matches!(&err, ResetCursorsError::PipelineRunning { run_id: held } if *held == run_id),
            "force={force}: expected PipelineRunning naming the run, got {err:?}"
        );
    }

    assert_eq!(
        high_water_of(&store.load().await.unwrap().state, "vendor_sales").as_deref(),
        Some("2026-09-20"),
        "a refused reset must not have cleared anything"
    );
    assert!(
        matches!(
            pipeline_lease::try_acquire(&db, WORKSPACE_ID, &name, "someone-else", LEASE_TTL_SECS)
                .await
                .unwrap(),
            LeaseAcquisition::Held { run_id: held, .. } if held == run_id
        ),
        "the refused reset must leave the run's lease with the run"
    );
}

/// The reset holds the lease only for itself. Leaked, it would defer the
/// pipeline's next run for the lease's TTL — on the success path and on the
/// refusal path alike.
#[tokio::test(flavor = "multi_thread")]
async fn a_cursor_reset_releases_the_lease_it_took() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let name = format!("amazon_vc_{}", Uuid::new_v4().simple());
    let (executor, _store) = seeded(&db, &name).await;
    let next_run_can_start = |label: &'static str| {
        let db = db.clone();
        let name = name.clone();
        async move {
            let run_id = format!("run-{}", Uuid::new_v4());
            assert_eq!(
                pipeline_lease::try_acquire(&db, WORKSPACE_ID, &name, &run_id, LEASE_TTL_SECS)
                    .await
                    .unwrap(),
                LeaseAcquisition::Acquired,
                "{label}: the next run must be able to take the lease"
            );
            pipeline_lease::release_counted(&db, WORKSPACE_ID, &name, &run_id)
                .await
                .unwrap();
        }
    };

    executor
        .reset_airway_cursors(
            "pipelines/amazon_vc.airway.yml",
            &CursorScope::Resources(vec!["vendor_sales".into()]),
            false,
        )
        .await
        .expect("scoped reset");
    next_run_can_start("after a reset that cleared").await;

    let err = executor
        .reset_airway_cursors(
            "pipelines/amazon_vc.airway.yml",
            &CursorScope::AllResources,
            false,
        )
        .await
        .expect_err("vendor_forecasting appends");
    assert!(matches!(err, ResetCursorsError::Refused(_)), "{err:?}");
    next_run_can_start("after a reset that was refused").await;
}

/// `force` is the operator asserting the judgement is wrong for their case.
/// It must actually override, or the refusal becomes a wall rather than a
/// guard rail.
#[tokio::test(flavor = "multi_thread")]
async fn force_overrides_the_refusal() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let name = format!("amazon_vc_{}", Uuid::new_v4().simple());
    let (executor, store) = seeded(&db, &name).await;

    let cleared = executor
        .reset_airway_cursors(
            "pipelines/amazon_vc.airway.yml",
            &CursorScope::AllResources,
            true,
        )
        .await
        .expect("force must override the convergence refusal");
    assert_eq!(
        cleared.cleared,
        vec!["vendor_forecasting".to_string(), "vendor_sales".to_string()]
    );
    assert!(
        store.load().await.unwrap().state.resource_states.is_empty(),
        "forced reset clears every cursor"
    );
}

/// A cursor reset resolves no destination, so — unlike Reset schema, which is
/// airhouse-only because it must re-mint a credential to issue the drop — it
/// works on a pipeline whose `destination:` is a plain `config.yml` reference
/// this workspace cannot resolve at all. Pinned because "reuse
/// `reset_airway_schema`'s preamble" is the natural way to write this method,
/// and it would make every non-airhouse pipeline un-rewindable for no reason.
#[tokio::test(flavor = "multi_thread")]
async fn cursor_reset_needs_no_destination_credential() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let name = format!("amazon_vc_{}", Uuid::new_v4().simple());
    let (executor, _store) = seeded(&db, &name).await;

    // `FakeWorkspace::resolve_connector` returns `None` for every database, so
    // the `warehouse` reference in the fixture is unresolvable here.
    executor
        .reset_airway_cursors(
            "pipelines/amazon_vc.airway.yml",
            &CursorScope::Resources(vec!["vendor_sales".into()]),
            false,
        )
        .await
        .expect("an unresolvable destination must not block a cursor reset");
}

/// A bad `pipeline_ref` is the caller's mistake, and must stay distinguishable
/// from a refusal — one is a `400`, the other a `409`, and an operator retries
/// them very differently.
#[tokio::test(flavor = "multi_thread")]
async fn an_unparseable_spec_is_a_bad_request_not_a_refusal() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let platform: Arc<dyn agentic_pipeline::platform::PlatformContext> = Arc::new(FakeWorkspace {
        yaml: "name: broken\nsource: {}\n".to_string(),
    });
    let executor = PipelineTaskExecutor::bare(platform, db.clone());

    let err = executor
        .reset_airway_cursors(
            "pipelines/broken.airway.yml",
            &CursorScope::AllResources,
            false,
        )
        .await
        .expect_err("an unparseable spec must not succeed");
    assert!(
        matches!(err, ResetCursorsError::BadRequest(_)),
        "got {err:?}"
    );
}
