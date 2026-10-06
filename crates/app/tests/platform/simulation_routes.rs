//! The four simulation routes, driven end to end against a real database.
//!
//! These exist because Phase 1's acceptance is "verify through the API", and a
//! route that only compiles has not been verified. Two things they pin that
//! nothing else does:
//!
//! * **Tenant scoping.** `simulation_runs` is keyed by `run_id` alone, so every
//!   read has to carry its own `workspace_id` filter. A missing filter is a
//!   cross-tenant leak that no type catches and that a single-workspace test
//!   would never see — so every case here seeds two workspaces.
//! * **The enqueue contract.** The handler writes a `TaskSpec::Custom` payload
//!   that a *different* process deserializes into `SimulationRunPayload`. If
//!   those drift, runs fail at execution with a message about JSON.
//!
//! Runs the central migrator, so it also exercises the two simulation
//! migrations rather than trusting them to be well-formed.

use entity::workspaces::WorkspaceStatus;
use entity::{
    organizations, revisions, simulation_definitions, simulation_run_fits, simulation_run_periods,
    simulation_runs, workspaces,
};
use migration::MigratorTrait;
use oxy::config::model::Config;
use oxy::config::{ConfigBuilder, ConfigManager, Origin, WorkingCopy};
use oxy_app::server::api::simulation::{
    RunRequest, list_workspace_runs, list_worlds, parse_policies, read_run, start_run,
};
use oxy_app::server::simulation::{SIMULATION_RUN_KIND, SimulationRunPayload};
use oxy_simulation::PolicyKind;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait,
    PaginatorTrait, QueryFilter, Statement,
};
use serde_json::json;
use uuid::Uuid;

/// An org + workspace with **no promoted revision**, so the compile boundary
/// declines and the FS fallback is what answers.
///
/// This distinction is the whole bug: a promoted revision that simply has no
/// simulation rows is authoritatively empty, and falling back to disk there
/// would mask a genuinely empty grid. Only "there is no revision at all" means
/// read the working copy.
async fn seed_unpromoted_workspace(db: &DatabaseConnection, label: &str) -> Uuid {
    let now = chrono::Utc::now().fixed_offset();
    let org_id = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org_id),
        name: ActiveValue::Set(format!("{label}-org")),
        slug: ActiveValue::Set(format!("{label}-{}", org_id.simple())),
        logo: ActiveValue::NotSet,
        logo_content_type: ActiveValue::NotSet,
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(db)
    .await
    .expect("seed org");

    let ws_id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(ws_id),
        name: ActiveValue::Set(format!("{label}-ws")),
        git_namespace_id: ActiveValue::Set(None),
        git_remote_url: ActiveValue::Set(None),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
        path: ActiveValue::Set(None),
        last_opened_at: ActiveValue::Set(None),
        created_by: ActiveValue::Set(None),
        org_id: ActiveValue::Set(Some(org_id)),
        status: ActiveValue::Set(WorkspaceStatus::Ready),
        error: ActiveValue::Set(None),
        monthly_vlm_budget_micros: ActiveValue::Set(None),
        current_revision_id: ActiveValue::Set(None),
        default_branch: ActiveValue::Set(None),
        repo_subdir: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed workspace");
    ws_id
}

/// A workspace directory with no worlds on disk.
///
/// The FS fallback fires whenever the compile boundary declines, so a test that
/// means to exercise the *compiled* path has to point somewhere empty — else it
/// would silently be asserting on whatever happens to be in the repo.
fn empty_workspace() -> tempfile::TempDir {
    tempfile::TempDir::new().expect("temp workspace")
}

/// The manager `workspace_middleware` would have attached, stated directly.
///
/// `build_with_provided_config_and_working_copy` rather than a loading
/// terminal, because those downgrade `Origin::Compiled` to `Disk` when the
/// revision carries no `config.yml` row — which every workspace seeded here
/// does. The origin IS what these tests are about, so it has to be stated
/// rather than re-derived.
fn manager(origin: Origin, root: &std::path::Path) -> ConfigManager<WorkingCopy> {
    ConfigBuilder::new()
        .with_workspace_path(root)
        .expect("workspace path")
        .build_with_provided_config_and_working_copy(empty_config(), origin)
        .expect("manager")
}

/// A workspace with nothing declared in `config.yml` — none of these tests
/// reads a database or a model out of it.
fn empty_config() -> Config {
    serde_yaml::from_str("databases: []\nmodels: []\n").expect("empty config")
}

/// A promoted workspace's manager: reads the compile boundary.
fn compiled(
    workspace_id: Uuid,
    revision_id: Uuid,
    root: &std::path::Path,
) -> ConfigManager<WorkingCopy> {
    manager(
        Origin::Compiled {
            workspace_id,
            revision_id,
        },
        root,
    )
}

/// Per-test database, wired so the handlers' own `establish_connection()` lands
/// on it.
async fn setup_db() -> DatabaseConnection {
    let (db, test_url) = crate::common::fresh_db(crate::common::Schema::Central).await;
    // SAFETY: single-threaded test setup before any other env access. nextest
    // isolates each test in its own process.
    unsafe {
        std::env::set_var("OXY_DATABASE_URL", &test_url);
        std::env::remove_var("OXY_DATABASE_AUTH_MODE");
    }

    // `Schema::Central` runs the central migrator only, and the orchestrator
    // owns a SECOND migrator with its own tracking table
    // (`seaql_migrations_orchestrator`). Without it `agentic_runs` exists as an
    // eleven-column stub and every `insert_run` fails on a missing
    // `source_type` — which reads as a bug in the caller rather than as a
    // half-migrated database.
    agentic_runtime::migration::RuntimeMigrator::up(&db, None)
        .await
        .expect("run orchestrator migrations");
    db
}

/// An org + a promoted workspace, returning `(workspace_id, revision_id)`.
async fn seed_workspace(db: &DatabaseConnection, label: &str) -> (Uuid, Uuid) {
    let now = chrono::Utc::now().fixed_offset();
    let org_id = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org_id),
        name: ActiveValue::Set(format!("{label}-org")),
        slug: ActiveValue::Set(format!("{label}-{}", org_id.simple())),
        logo: ActiveValue::NotSet,
        logo_content_type: ActiveValue::NotSet,
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(db)
    .await
    .expect("seed org");

    let ws_id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(ws_id),
        name: ActiveValue::Set(format!("{label}-ws")),
        git_namespace_id: ActiveValue::Set(None),
        git_remote_url: ActiveValue::Set(None),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
        path: ActiveValue::Set(None),
        last_opened_at: ActiveValue::Set(None),
        created_by: ActiveValue::Set(None),
        org_id: ActiveValue::Set(Some(org_id)),
        status: ActiveValue::Set(WorkspaceStatus::Ready),
        error: ActiveValue::Set(None),
        monthly_vlm_budget_micros: ActiveValue::Set(None),
        current_revision_id: ActiveValue::Set(None),
        default_branch: ActiveValue::Set(None),
        repo_subdir: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed workspace");

    let rev_id = Uuid::new_v4();
    revisions::ActiveModel {
        revision_id: ActiveValue::Set(rev_id),
        workspace_id: ActiveValue::Set(ws_id),
        git_sha: ActiveValue::Set("deadbeef".into()),
        branch: ActiveValue::Set(Some("main".into())),
        schema_version: ActiveValue::Set(1),
        status: ActiveValue::Set("ready".into()),
        kind: ActiveValue::Set("full".into()),
        owner_user_id: ActiveValue::Set(None),
        compiler_version: ActiveValue::Set("test".into()),
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

    let mut ws: workspaces::ActiveModel = workspaces::Entity::find_by_id(ws_id)
        .one(db)
        .await
        .expect("load workspace")
        .expect("workspace exists")
        .into();
    ws.current_revision_id = ActiveValue::Set(Some(rev_id));
    ws.update(db).await.expect("promote revision");

    (ws_id, rev_id)
}

/// A compiled `.simulation.yml`, as the walker would have written it.
///
/// A whole world, not a stub: enqueueing reads the declared `seed` and
/// `replicates` to fan the request out, so a body that is not a parseable world
/// is a 400 rather than a queued run. A fixture that could not be one would be
/// testing a path no real workspace takes.
async fn seed_world(db: &DatabaseConnection, rev_id: Uuid, name: &str) {
    simulation_definitions::ActiveModel {
        revision_id: ActiveValue::Set(rev_id),
        name: ActiveValue::Set(name.into()),
        file_path: ActiveValue::Set(format!("simulations/{name}.simulation.yml")),
        definition: ActiveValue::Set(world_body(name)),
    }
    .insert(db)
    .await
    .expect("seed world");
}

/// The smallest coherent world, as JSON — the shape `oxy-compile` stores.
fn world_body(name: &str) -> serde_json::Value {
    json!({
        "name": name,
        "seed": 7,
        "periods": 30,
        "period_days": 7,
        "history_days": 180,
        "start_date": "2025-01-06",
        "entities": { "count": 24, "scale_sigma": 0.4 },
        "baseline": {
            "sales_per_entity_day": 1500.0,
            "margin": 0.36,
            "demand_shock_rho": 0.7,
            "demand_shock_sd": 0.12,
            "weekly_seasonality": 0.15
        },
        "mechanism": {
            "driver": "marketing_spend",
            "target": "net_sales",
            "lag_days": 7,
            "noise_ratio": 0.05,
            "calibrate": {
                "anchor_spend_share": 0.02,
                "local_slope_at_anchor": 4.0,
                "optimum_at": 3.0
            }
        }
    })
}

/// A finished run with one period and one scored edge.
async fn seed_run(db: &DatabaseConnection, ws_id: Uuid) -> Uuid {
    let now = chrono::Utc::now().fixed_offset();
    let run_id = Uuid::new_v4();
    simulation_runs::ActiveModel {
        run_id: ActiveValue::Set(run_id),
        workspace_id: ActiveValue::Set(ws_id),
        revision_id: ActiveValue::Set(None),
        simulation_name: ActiveValue::Set("confounded".into()),
        policy: ActiveValue::Set("machine".into()),
        seed: ActiveValue::Set(7),
        replicate: ActiveValue::Set(0),
        status: ActiveValue::Set("done".into()),
        spec: ActiveValue::Set(json!({ "name": "confounded" })),
        truth: ActiveValue::Set(Some(json!({ "theta": 0.668 }))),
        periods_planned: ActiveValue::Set(2),
        periods_done: ActiveValue::Set(2),
        queued_at: ActiveValue::Set(now),
        started_at: ActiveValue::Set(now),
        finished_at: ActiveValue::Set(Some(now)),
        error: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed run");

    // Inserted out of order on purpose: the reader must sort, not rely on
    // insertion order, or a stepper scrubs through time backwards.
    for period in [2i32, 1] {
        simulation_run_periods::ActiveModel {
            run_id: ActiveValue::Set(run_id),
            period: ActiveValue::Set(period),
            mean_spend: ActiveValue::Set(38.0 + period as f64),
            realized_profit: ActiveValue::Set(100.0 * period as f64),
            cumulative_profit: ActiveValue::Set(100.0 * period as f64),
            actions: ActiveValue::Set(json!([38.0, 39.0])),
        }
        .insert(db)
        .await
        .expect("seed period");

        simulation_run_fits::ActiveModel {
            run_id: ActiveValue::Set(run_id),
            period: ActiveValue::Set(period),
            edge: ActiveValue::Set("store_days.marketing_spend -> store_days.net_sales".into()),
            form: ActiveValue::Set("log-log".into()),
            // Null on a refusal — the distinction the outcome taxonomy turns on,
            // and it has to survive the round-trip through Postgres.
            coefficient: ActiveValue::Set(if period == 1 { None } else { Some(4.6) }),
            se: ActiveValue::Set(if period == 1 { None } else { Some(0.02) }),
            t_stat: ActiveValue::Set(if period == 1 { None } else { Some(230.0) }),
            n: ActiveValue::Set(4152),
            n_panels: ActiveValue::Set(24),
            refusal: ActiveValue::Set(if period == 1 {
                Some("abs t < 2".into())
            } else {
                None
            }),
            true_local_slope: ActiveValue::Set(3.7),
            outcome: ActiveValue::Set(if period == 1 { "refused" } else { "converged" }.into()),
        }
        .insert(db)
        .await
        .expect("seed fit");
    }
    run_id
}

#[tokio::test]
async fn lists_the_worlds_the_revision_declares() {
    let db = setup_db().await;
    let (ws_id, rev_id) = seed_workspace(&db, "list").await;
    seed_world(&db, rev_id, "confounded").await;
    seed_world(&db, rev_id, "clean").await;

    let root = empty_workspace();
    let out = list_worlds(&compiled(ws_id, rev_id, root.path()))
        .await
        .expect("list");
    let mut names: Vec<_> = out.iter().map(|s| s.name.clone()).collect();
    names.sort();
    assert_eq!(names, vec!["clean", "confounded"]);
    assert!(
        out[0].file_path.ends_with(".simulation.yml"),
        "file_path was not carried through"
    );
}

#[tokio::test]
async fn a_workspace_with_no_declared_worlds_gets_an_empty_list_not_an_error() {
    // A workspace that has never compiled has no worlds. That is a fact about
    // the workspace, not a failure — and returning 500 here would make the
    // surface look broken on every fresh install.
    let db = setup_db().await;
    let (ws_id, rev_id) = seed_workspace(&db, "empty").await;
    let root = empty_workspace();
    let out = list_worlds(&compiled(ws_id, rev_id, root.path()))
        .await
        .expect("list");
    assert!(out.is_empty());
}

#[tokio::test]
async fn enqueueing_an_unknown_world_is_a_404_not_a_queued_run() {
    let db = setup_db().await;
    let (ws_id, rev_id) = seed_workspace(&db, "unknown").await;
    seed_world(&db, rev_id, "confounded").await;

    let root = empty_workspace();
    let err = start_run(
        ws_id,
        &compiled(ws_id, rev_id, root.path()),
        RunRequest::new("no_such_world"),
    )
    .await
    .expect_err("should not have queued anything");
    assert_eq!(err.0, axum::http::StatusCode::NOT_FOUND);

    let queued = simulation_runs::Entity::find()
        .all(&db)
        .await
        .expect("count runs");
    assert!(
        queued.is_empty(),
        "a run row was created for a missing world"
    );
}

#[tokio::test]
async fn enqueueing_writes_a_payload_the_executor_can_read_back() {
    // The contract between two processes. The handler writes a
    // `TaskSpec::Custom` payload; a worker deserializes it into
    // `SimulationRunPayload`. Nothing but this test connects the two.
    let db = setup_db().await;
    let (ws_id, rev_id) = seed_workspace(&db, "enqueue").await;
    seed_world(&db, rev_id, "confounded").await;

    let root = empty_workspace();
    let queued = start_run(
        ws_id,
        &compiled(ws_id, rev_id, root.path()),
        RunRequest::new("confounded"),
    )
    .await
    .expect("enqueue");
    // One arm, one draw: a caller who names only a world gets the product on the
    // world's own seed.
    assert_eq!(queued.len(), 1);
    let queued = &queued[0];
    assert_eq!(queued.simulation, "confounded");
    assert_eq!(queued.policy, "machine");
    assert_eq!(queued.replicate, 0);
    assert_eq!(queued.seed, 7, "replicate 0 must run the declared seed");

    let row = db
        .query_one_raw(Statement::from_string(
            db.get_database_backend(),
            format!(
                "SELECT spec::text AS spec FROM agentic_task_queue WHERE task_id = '{}'",
                queued.run_id
            ),
        ))
        .await
        .expect("query queue")
        .expect("the task was not enqueued");
    let spec: String = row.try_get("", "spec").expect("spec column");
    let spec: serde_json::Value = serde_json::from_str(&spec).expect("spec is not JSON");

    assert_eq!(spec["type"], "custom", "not a Custom TaskSpec: {spec}");
    assert_eq!(spec["kind"], SIMULATION_RUN_KIND);
    let payload: SimulationRunPayload =
        serde_json::from_value(spec["payload"].clone()).expect("payload does not round-trip");
    assert_eq!(payload.run_id, queued.run_id);
    assert_eq!(payload.workspace_id, ws_id);
    assert_eq!(payload.policy, PolicyKind::Machine);
    assert_eq!(payload.replicate, 0);
    // The spec travels by value, so a later edit to the file cannot change what
    // this run executes.
    assert_eq!(payload.spec["name"], "confounded");
}

#[tokio::test]
async fn a_run_is_readable_the_instant_it_is_queued_not_once_a_worker_claims_it() {
    // The bug: `GET .../runs/{run_id}` used to 404 with "no such run" until a
    // worker claimed the task and wrote the row — a real gap, since a missed
    // Postgres NOTIFY falls back to the worker's poll interval. The row now
    // has to exist synchronously, before this function ever returns.
    let db = setup_db().await;
    let (ws_id, rev_id) = seed_workspace(&db, "queued-read").await;
    seed_world(&db, rev_id, "confounded").await;

    let root = empty_workspace();
    let queued = start_run(
        ws_id,
        &compiled(ws_id, rev_id, root.path()),
        RunRequest::new("confounded"),
    )
    .await
    .expect("enqueue");
    let run_id = queued[0].run_id;

    let detail = read_run(ws_id, run_id)
        .await
        .expect("a just-queued run must be readable, not 404");
    assert_eq!(detail.run.status, "queued");
    assert_eq!(detail.run.periods_done, 0);
    assert_eq!(
        detail.run.periods_planned, 30,
        "the declared world's periods"
    );
    assert!(detail.periods.is_empty());
}

#[tokio::test]
async fn a_run_is_readable_with_its_periods_and_fits_in_order() {
    let db = setup_db().await;
    let (ws_id, _) = seed_workspace(&db, "read").await;
    let run_id = seed_run(&db, ws_id).await;

    let detail = read_run(ws_id, run_id).await.expect("get run");
    assert_eq!(detail.run.run_id, run_id);
    assert_eq!(
        detail.periods.iter().map(|p| p.period).collect::<Vec<_>>(),
        vec![1, 2],
        "periods came back out of order — a stepper would scrub backwards"
    );

    // The refusal must survive the round-trip as an absent coefficient, not a
    // zero. This is the distinction the whole outcome taxonomy turns on.
    let refused = detail
        .fits
        .iter()
        .find(|f| f.period == 1)
        .expect("period 1");
    assert_eq!(refused.coefficient, None);
    assert_eq!(refused.outcome, "refused");
    assert_eq!(refused.refusal.as_deref(), Some("abs t < 2"));
    let converged = detail
        .fits
        .iter()
        .find(|f| f.period == 2)
        .expect("period 2");
    assert_eq!(converged.coefficient, Some(4.6));
    assert_eq!(converged.form, "log-log");
    assert_eq!(converged.true_local_slope, 3.7);
}

#[tokio::test]
async fn a_run_belonging_to_another_workspace_is_not_readable() {
    // The correctness invariant. `simulation_runs` is keyed by `run_id` alone,
    // so nothing but the handler's own filter stops a caller reading another
    // tenant's run by guessing — or by holding a stale id.
    let db = setup_db().await;
    let (mine, _) = seed_workspace(&db, "mine").await;
    let (theirs, _) = seed_workspace(&db, "theirs").await;
    let their_run = seed_run(&db, theirs).await;

    let err = read_run(mine, their_run)
        .await
        .expect_err("read another tenant's run");
    assert_eq!(err.0, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn listing_runs_is_scoped_to_the_calling_workspace() {
    let db = setup_db().await;
    let (mine, _) = seed_workspace(&db, "mine-list").await;
    let (theirs, _) = seed_workspace(&db, "theirs-list").await;
    let my_run = seed_run(&db, mine).await;
    let _their_run = seed_run(&db, theirs).await;

    let listed = list_workspace_runs(mine).await.expect("list runs");
    assert_eq!(
        listed.iter().map(|r| r.run_id).collect::<Vec<_>>(),
        vec![my_run],
        "the listing crossed a tenant boundary"
    );
}

/// Write a minimal world to `<root>/simulations/<name>.simulation.yml`.
fn write_world(root: &std::path::Path, name: &str) {
    let dir = root.join("simulations");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join(format!("{name}.simulation.yml")),
        // A whole world, for the same reason `world_body` is: enqueueing reads
        // the declared seed and replicate count, so a stub cannot be run.
        format!(
            "{}\n",
            serde_yaml::to_string(&world_body(name)).expect("world as yaml")
        ),
    )
    .expect("write world");
}

#[tokio::test]
async fn a_workspace_with_no_compiled_revision_reads_its_worlds_off_disk() {
    // The bug this exists for: "no promoted revision" means "read the
    // filesystem", not "there is no data". Answering an empty list there
    // reported "0 declared worlds" to everyone in the IDE — the local
    // workspace, a non-default branch, a workspace nobody has compiled yet,
    // i.e. the ordinary state of the surface this feature lives on.
    //
    // The decision itself now lives one layer up: `workspace_middleware`
    // resolves a workspace with no revision to `Origin::Disk`, which is the
    // manager these cases hand the reader. No database is touched on this arm.
    let root = empty_workspace();
    write_world(root.path(), "flat_lever");
    write_world(root.path(), "marketing_lift");

    let out = list_worlds(&manager(Origin::Disk, root.path()))
        .await
        .expect("list");
    assert_eq!(
        out.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(),
        vec!["flat_lever", "marketing_lift"],
        "the working copy's worlds were not read"
    );
    assert_eq!(
        out[0].file_path, "simulations/flat_lever.simulation.yml",
        "file_path must stay workspace-relative"
    );
}

#[tokio::test]
async fn a_world_only_on_disk_can_still_be_run() {
    // The listing and the run have to agree about what exists. Resolving only
    // through the compile boundary would 404 every world the page just listed.
    let db = setup_db().await;
    let ws_id = seed_unpromoted_workspace(&db, "fs-run").await;
    let root = empty_workspace();
    write_world(root.path(), "marketing_lift");

    let queued = start_run(
        ws_id,
        &manager(Origin::Disk, root.path()),
        RunRequest::new("marketing_lift"),
    )
    .await
    .expect("a world listed off disk must be runnable");
    let queued = &queued[0];
    assert_eq!(queued.simulation, "marketing_lift");

    // And the payload carries the on-disk body, not an empty spec.
    let row = db
        .query_one_raw(Statement::from_string(
            db.get_database_backend(),
            format!(
                "SELECT spec::text AS spec FROM agentic_task_queue WHERE task_id = '{}'",
                queued.run_id
            ),
        ))
        .await
        .expect("query queue")
        .expect("not enqueued");
    let spec: serde_json::Value =
        serde_json::from_str(&row.try_get::<String>("", "spec").unwrap()).unwrap();
    assert_eq!(spec["payload"]["spec"]["name"], "marketing_lift");
}

#[tokio::test]
async fn build_and_vcs_directories_are_skipped_on_disk() {
    // Same skip set as the compile walker. A stray copy under `target/` would
    // list as a second world with the same name, indistinguishable from a real
    // one — the duplicate-view-name failure, one surface over.
    let root = empty_workspace();
    write_world(root.path(), "marketing_lift");
    write_world(&root.path().join("target/debug"), "marketing_lift");
    write_world(&root.path().join(".worktrees/wip"), "marketing_lift");

    let out = list_worlds(&manager(Origin::Disk, root.path()))
        .await
        .expect("list");
    assert_eq!(out.len(), 1, "build/VCS copies leaked in: {out:?}");
}

#[tokio::test]
async fn one_unparseable_world_does_not_hide_the_others() {
    let root = empty_workspace();
    write_world(root.path(), "good");
    std::fs::write(
        root.path().join("simulations/broken.simulation.yml"),
        "name: broken\n  bad indent: [",
    )
    .expect("write broken");

    let out = list_worlds(&manager(Origin::Disk, root.path()))
        .await
        .expect("list");
    assert_eq!(
        out.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(),
        vec!["good"]
    );
}

/// Extractors in `src` that take a single path segment.
///
/// Split out so the detector itself is testable — a scan that silently matches
/// nothing is worse than no scan, because it reads as a passing guard.
fn single_segment_path_extractors(src: &str) -> Vec<&str> {
    src.lines()
        .map(str::trim)
        .filter(|l| l.starts_with("Path(") && !l.starts_with("Path((") && l.contains("):"))
        .collect()
}

/// Every `Path` extractor in a workspace-mounted handler must take the
/// `{workspace_id}` segment.
///
/// This is a source scan rather than a request test because the handler tests
/// above call `list_worlds` / `start_run` directly — they deliberately skip the
/// transport layer, which is exactly where this fails. A bare `Path<String>`
/// under a router mounted at `/{workspace_id}` compiles fine, passes every
/// unit test, and rejects the request at runtime with "Wrong number of path
/// arguments for `Path`. Expected 1 but got 2" before the handler body runs.
#[test]
fn path_extractors_account_for_the_workspace_id_segment() {
    // Both halves of the surface, because a handler moving between them must not
    // move out from under this scan.
    let src = concat!(
        include_str!("../../src/server/api/simulation/runs.rs"),
        include_str!("../../src/server/api/simulation/worlds.rs"),
    );

    // The detector has to actually see this file's extractors, or the assertion
    // below passes because it matched nothing.
    assert!(
        src.contains("Path((_workspace_id, name)): Path<(Uuid, String)>"),
        "the handlers moved; this scan is now looking at the wrong thing"
    );
    assert_eq!(
        single_segment_path_extractors(
            "    Path(run_id): Path<Uuid>,\n    Path((a, b)): Path<(Uuid, Uuid)>,"
        ),
        vec!["Path(run_id): Path<Uuid>,"],
        "the detector does not flag a bare single-segment extractor"
    );

    let offenders = single_segment_path_extractors(src);
    assert!(
        offenders.is_empty(),
        "these Path extractors take a single segment, but the workspace router \
         mounts under /{{workspace_id}} — use Path<(Uuid, _)>: {offenders:?}"
    );
}

#[tokio::test]
async fn racing_two_arms_queues_two_runs_of_one_world() {
    // The fix the whole arm-out-of-the-file change exists for: `hold` and
    // `machine` over ONE world on ONE seed. When each arm was its own
    // `.simulation.yml`, the two could drift apart in review and the profit race
    // silently became a comparison between two different worlds.
    let db = setup_db().await;
    let (ws_id, rev_id) = seed_workspace(&db, "race").await;
    seed_world(&db, rev_id, "confounded").await;

    let root = empty_workspace();
    let queued = start_run(
        ws_id,
        &compiled(ws_id, rev_id, root.path()),
        RunRequest {
            name: "confounded",
            policies: vec![PolicyKind::Hold, PolicyKind::Machine],
            replicates: None,
        },
    )
    .await
    .expect("enqueue");

    assert_eq!(queued.len(), 2);
    assert_eq!(
        queued.iter().map(|r| r.policy.as_str()).collect::<Vec<_>>(),
        vec!["hold", "machine"]
    );
    assert!(
        queued.iter().all(|r| r.seed == 7),
        "the arms of a race must see the same world: {:?}",
        queued.iter().map(|r| r.seed).collect::<Vec<_>>()
    );
    assert_ne!(queued[0].run_id, queued[1].run_id);
}

#[tokio::test]
async fn replicates_fan_out_onto_distinct_seeds() {
    // A cell of the outcome map is an aggregate over draws, so the draws have to
    // actually differ — and each run's stored spec has to carry the seed it ran,
    // not the file's, or a reader cannot tell which world any given row saw.
    let db = setup_db().await;
    let (ws_id, rev_id) = seed_workspace(&db, "replicates").await;
    seed_world(&db, rev_id, "confounded").await;

    let root = empty_workspace();
    let queued = start_run(
        ws_id,
        &compiled(ws_id, rev_id, root.path()),
        RunRequest {
            name: "confounded",
            policies: vec![PolicyKind::Machine],
            replicates: Some(3),
        },
    )
    .await
    .expect("enqueue");

    let seeds: Vec<u64> = queued.iter().map(|r| r.seed).collect();
    assert_eq!(seeds, vec![7, 8, 9]);
    assert_eq!(
        queued.iter().map(|r| r.replicate).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );

    for run in &queued {
        let row = db
            .query_one_raw(Statement::from_string(
                db.get_database_backend(),
                format!(
                    "SELECT spec::text AS spec FROM agentic_task_queue WHERE task_id = '{}'",
                    run.run_id
                ),
            ))
            .await
            .expect("query queue")
            .expect("the task was not enqueued");
        let spec: String = row.try_get("", "spec").expect("spec column");
        let spec: serde_json::Value = serde_json::from_str(&spec).expect("spec is not JSON");
        let payload: SimulationRunPayload =
            serde_json::from_value(spec["payload"].clone()).expect("payload does not round-trip");
        assert_eq!(
            payload.spec["seed"], run.seed,
            "the snapshot carries the file's seed rather than this draw's"
        );
    }
}

#[test]
fn an_unknown_arm_is_refused_rather_than_defaulted() {
    // The arms arrive as a query string, so this is where a typo would be
    // absorbed. Queueing `machine` when someone asked for something else would
    // put a number on the profit race with nothing to say it is the wrong arm.
    let err = parse_policies(Some("hold,nonsense"))
        .expect_err("an unknown arm must not be silently dropped");
    assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    assert!(err.1.contains("nonsense"), "unhelpful error: {}", err.1);
    assert!(
        err.1.contains("machine_explore"),
        "the error should name the arms that do exist: {}",
        err.1
    );

    assert_eq!(
        parse_policies(Some("hold, machine")).unwrap(),
        vec![PolicyKind::Hold, PolicyKind::Machine],
        "whitespace after the comma is not a typo"
    );
    // No parameter at all means the default arm, resolved in `start_run` rather
    // than here — an empty list, not a defaulted one.
    assert!(parse_policies(None).unwrap().is_empty());
}

// ── status semantics: "could not look" is not "found nothing" ────────────
//
// Nothing pinned these before, which is how a rewrite of the read path could
// silently move a 503 to a 500 or a 404 to a 503 and stay green. The
// distinction is the whole reason `ArtifactError::retryable()` exists: a
// caller retries a 503 and gives up on a 404, so a node that could not look
// must never answer as though it had looked.

/// A node with `Origin::Disk` and nothing to read asks for a retry rather than
/// reporting an empty grid.
///
/// This is the mid-deploy shape: the manager was told to read the files and
/// the files are not here. Answering `[]` would report a platform-side fault
/// as "this workspace declares no worlds", which is exactly the conflation
/// `no_source_to_read` exists to refuse.
#[tokio::test]
async fn a_node_with_no_files_to_read_asks_for_a_retry_not_an_empty_grid() {
    let root = empty_workspace();
    let err = list_worlds(&manager(Origin::Disk, root.path()).without_working_copy())
        .await
        .expect_err("a node with nothing to read must not answer `[]`");
    assert_eq!(
        err.0,
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        "a node that could not look answered as though it had: {err:?}"
    );
}

/// A promoted revision that declares no worlds is authoritatively empty, and
/// says so with a 200 — the counterpart the case above would otherwise be
/// indistinguishable from.
#[tokio::test]
async fn a_replica_reading_a_promoted_but_empty_revision_answers_a_plain_empty_list() {
    let db = setup_db().await;
    let (ws_id, rev_id) = seed_workspace(&db, "empty-promoted").await;

    let root = empty_workspace();
    let out = list_worlds(&compiled(ws_id, rev_id, root.path()).without_working_copy())
        .await
        .expect("an empty promoted revision is an answer, not a failure");
    assert!(out.is_empty(), "expected an empty grid, got {out:?}");
}

/// On a replica, an unknown world name is a RETRY, not a 404.
///
/// Deliberate and inherited rather than local: `ConfigManager` reports an
/// absent compiled row as `NoSource` on a node with no working copy —
/// documented on `root_singleton` — because the compile may simply not have
/// promoted yet, and telling a caller "no such world" about a world that is
/// about to exist is worse than telling it to ask again. Pinned here because
/// it reads as a bug next to the 404 below, and the next person to "fix" it
/// should have to delete a test that says why.
#[tokio::test]
async fn an_unknown_world_on_a_replica_asks_for_a_retry_rather_than_denying_it_exists() {
    let db = setup_db().await;
    let (ws_id, rev_id) = seed_workspace(&db, "replica-unknown").await;
    seed_world(&db, rev_id, "confounded").await;

    let root = empty_workspace();
    let err = start_run(
        ws_id,
        &compiled(ws_id, rev_id, root.path()).without_working_copy(),
        RunRequest::new("no_such_world"),
    )
    .await
    .expect_err("should not have queued anything");
    assert_eq!(
        err.0,
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        "a replica denied a world it could not have looked for: {err:?}"
    );

    let queued = simulation_runs::Entity::find()
        .all(&db)
        .await
        .expect("count runs");
    assert!(
        queued.is_empty(),
        "a run row was created for a missing world"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_in_flight_cap_holds_when_requests_arrive_together() {
    // The cap is the only thing standing between a member and an unbounded
    // amount of fleet work: every run is minutes of blocking DuckDB and fitter
    // CPU, and the route is reachable by any workspace member. A cap that only
    // holds when requests arrive one at a time is not a cap — it is a bound of
    // `MAX_IN_FLIGHT_PER_WORKSPACE × concurrency`, which is whatever the caller
    // chooses to make it.
    //
    // Eight requests of sixteen runs each is 128 against a cap of 64, all
    // reading the count before any of them has committed a row.
    let db = setup_db().await;
    let (ws_id, rev_id) = seed_workspace(&db, "cap-race").await;
    seed_world(&db, rev_id, "confounded").await;
    let root = empty_workspace();

    const REQUESTS: usize = 8;
    const PER_REQUEST: u32 = 16;

    let attempts = (0..REQUESTS).map(|_| {
        let manager = compiled(ws_id, rev_id, root.path());
        async move {
            start_run(
                ws_id,
                &manager,
                RunRequest {
                    name: "confounded",
                    policies: vec![PolicyKind::Machine],
                    replicates: Some(PER_REQUEST),
                },
            )
            .await
        }
    });
    let outcomes = futures::future::join_all(attempts).await;

    let queued_here: usize = outcomes
        .iter()
        .map(|o| o.as_ref().map(|q| q.len()).unwrap_or(0))
        .sum();
    let rows = simulation_runs::Entity::find()
        .filter(simulation_runs::Column::WorkspaceId.eq(ws_id))
        .count(&db)
        .await
        .expect("count runs");

    assert_eq!(
        rows as usize, queued_here,
        "every run the responses claim must be a row, and no row may be unclaimed"
    );
    assert!(
        rows <= oxy_app::server::api::simulation::MAX_IN_FLIGHT_PER_WORKSPACE,
        "{REQUESTS} concurrent requests of {PER_REQUEST} queued {rows} runs, over the cap of {}",
        oxy_app::server::api::simulation::MAX_IN_FLIGHT_PER_WORKSPACE
    );
    assert!(
        rows > 0,
        "the cap must admit the first request, not refuse everything"
    );
}

// ── the paired profit race ────────────────────────────────────────────────────
//
// `GET /simulations/{name}/race` is the read that makes `oxy_simulation::race`
// reachable in bulk. Everything it asserts is a property nothing else can:
// per-replicate profit lives one row per period in `simulation_run_periods`,
// and the arms of a race are separate rows in `simulation_runs`, so the only
// place the two meet is this query.
//
// Tenant scoping is asserted the same way every case above does it — two
// workspaces, both seeded with the same world name, because `simulation_runs`
// is keyed by `run_id` alone.

/// One run of one arm, stated in full: which world it drew, which status it
/// stopped (or did not stop) at, and when it was queued relative to its
/// siblings.
///
/// A struct rather than eight positionals, and every field explicit, because
/// the three the race actually keys on — `seed`, `status`, and the queueing
/// order — are precisely the ones a positional call would bury.
#[derive(Clone, Copy)]
struct RaceRunSeed<'a> {
    world: &'a str,
    policy: &'a str,
    /// The human-facing label. NOT the pairing key.
    replicate: i32,
    /// The world identity. Replicate `k` of two arms is the same world only
    /// when this matches.
    seed: i64,
    /// `queued` | `running` | `done` | `failed` | `cancelled` — the five
    /// `crates/app/src/server/simulation/store.rs` writes.
    status: &'a str,
    /// Seconds after the case's base clock. Higher is newer, and newest wins
    /// deduplication — so a re-queue is `queued_offset_secs` above the run it
    /// supersedes, stated rather than left to `Utc::now()` resolution.
    queued_offset_secs: i64,
    /// `(period, cumulative_profit)`. Empty for a run that died before its
    /// first period, which is a case the horizon rule has to survive rather
    /// than divide by.
    periods: &'a [(i32, f64)],
}

impl<'a> Default for RaceRunSeed<'a> {
    fn default() -> Self {
        Self {
            world: "w",
            policy: "machine",
            replicate: 0,
            seed: 7,
            status: "done",
            queued_offset_secs: 0,
            periods: &[],
        }
    }
}

/// The base clock every `queued_offset_secs` is measured from. Fixed rather
/// than `now()` so ordering inside one case is decided by the offsets and
/// nothing else.
fn race_clock() -> chrono::DateTime<chrono::FixedOffset> {
    chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z").expect("race clock")
}

async fn seed_race_run_as(db: &DatabaseConnection, ws_id: Uuid, run: RaceRunSeed<'_>) -> Uuid {
    let queued_at = race_clock() + chrono::Duration::seconds(run.queued_offset_secs);
    let terminal = matches!(run.status, "done" | "failed" | "cancelled");
    let run_id = Uuid::new_v4();
    simulation_runs::ActiveModel {
        run_id: ActiveValue::Set(run_id),
        workspace_id: ActiveValue::Set(ws_id),
        revision_id: ActiveValue::Set(None),
        simulation_name: ActiveValue::Set(run.world.into()),
        policy: ActiveValue::Set(run.policy.into()),
        seed: ActiveValue::Set(run.seed),
        replicate: ActiveValue::Set(run.replicate),
        status: ActiveValue::Set(run.status.into()),
        spec: ActiveValue::Set(json!({ "name": run.world, "seed": run.seed })),
        truth: ActiveValue::Set(None),
        periods_planned: ActiveValue::Set(3),
        periods_done: ActiveValue::Set(run.periods.len() as i32),
        queued_at: ActiveValue::Set(queued_at),
        started_at: ActiveValue::Set(queued_at),
        finished_at: ActiveValue::Set(terminal.then_some(queued_at)),
        error: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed race run");

    for (period, cumulative) in run.periods {
        simulation_run_periods::ActiveModel {
            run_id: ActiveValue::Set(run_id),
            period: ActiveValue::Set(*period),
            mean_spend: ActiveValue::Set(40.0),
            realized_profit: ActiveValue::Set(*cumulative),
            cumulative_profit: ActiveValue::Set(*cumulative),
            actions: ActiveValue::Set(json!([40.0])),
        }
        .insert(db)
        .await
        .expect("seed race period");
    }
    run_id
}

/// The common shape: a terminal run of one draw, on the base-7 seed ladder the
/// fan-out would have produced (`replicate_seed(7, k) = 7 + k`), so replicate
/// `k` of every arm really is one world.
async fn seed_race_run(
    db: &DatabaseConnection,
    ws_id: Uuid,
    world: &str,
    policy: &str,
    replicate: i32,
    periods: &[(i32, f64)],
) -> Uuid {
    seed_race_run_as(
        db,
        ws_id,
        RaceRunSeed {
            world,
            policy,
            replicate,
            seed: 7 + replicate as i64,
            status: if periods.is_empty() { "failed" } else { "done" },
            periods,
            ..Default::default()
        },
    )
    .await
}

/// The whole read path, on a world raced across two arms and four draws.
///
/// The seed is deliberately ragged, because a real race is:
///
/// * `hold` #3 stops at period 2 — so period 2 is the deepest period EVERY
///   scored replicate reached, and 2 is what the race must be run at. Scoring
///   each run at its own last row would race `hold` #0 at period 3 against
///   `hold` #3 at period 2, which is not a comparison at all.
/// * `machine` #3 recorded nothing — a run that died before its first period.
///   It cannot set a horizon and it cannot be scored, so it is dropped and
///   counted rather than dividing by an empty curve.
/// * `machine` #4 has no `hold` twin, so it is unpaired — the case
///   `ArmProfits`' keying exists for.
#[tokio::test]
async fn a_profit_race_pairs_arms_at_the_deepest_period_every_replicate_reached() {
    let db = setup_db().await;
    let (mine, _) = seed_workspace(&db, "race-mine").await;
    let (theirs, _) = seed_workspace(&db, "race-theirs").await;

    // hold: three full draws plus one that stopped early.
    seed_race_run(
        &db,
        mine,
        "w",
        "hold",
        0,
        &[(1, 10.0), (2, 20.0), (3, 30.0)],
    )
    .await;
    seed_race_run(
        &db,
        mine,
        "w",
        "hold",
        1,
        &[(1, 12.0), (2, 24.0), (3, 36.0)],
    )
    .await;
    seed_race_run(&db, mine, "w", "hold", 2, &[(1, 8.0), (2, 16.0), (3, 24.0)]).await;
    seed_race_run(&db, mine, "w", "hold", 3, &[(1, 5.0), (2, 9.0)]).await;
    // machine: the same three draws, one that never produced a period, and one
    // draw hold never ran.
    seed_race_run(
        &db,
        mine,
        "w",
        "machine",
        0,
        &[(1, 11.0), (2, 22.0), (3, 45.0)],
    )
    .await;
    seed_race_run(
        &db,
        mine,
        "w",
        "machine",
        1,
        &[(1, 13.0), (2, 27.0), (3, 50.0)],
    )
    .await;
    seed_race_run(
        &db,
        mine,
        "w",
        "machine",
        2,
        &[(1, 9.0), (2, 20.0), (3, 30.0)],
    )
    .await;
    seed_race_run(&db, mine, "w", "machine", 3, &[]).await;
    seed_race_run(&db, mine, "w", "machine", 4, &[(1, 7.0), (2, 14.0)]).await;

    // Another tenant's runs of a world with the SAME name, and profits chosen so
    // that any leak would move every number below.
    for replicate in 0..4 {
        seed_race_run(
            &db,
            theirs,
            "w",
            "hold",
            replicate,
            &[(1, 1000.0), (2, 2000.0)],
        )
        .await;
        seed_race_run(
            &db,
            theirs,
            "w",
            "machine",
            replicate,
            &[(1, -1000.0), (2, -2000.0)],
        )
        .await;
    }
    // And a different world of mine, which must not be raced into this one.
    seed_race_run(&db, mine, "other", "machine", 0, &[(1, 999.0), (2, 999.0)]).await;

    let report = oxy_app::server::api::simulation::profit_race_report(
        mine,
        "w",
        oxy_app::server::api::simulation::RaceOptions::default(),
    )
    .await
    .expect("race");

    assert_eq!(report.simulation, "w");
    assert_eq!(
        report.horizon,
        Some(2),
        "the race must run at the deepest period every scored replicate reached"
    );
    assert_eq!(report.horizon_pinned, false);
    assert_eq!(report.baseline.as_deref(), Some("hold"));

    // One challenger, so one comparison.
    assert_eq!(report.comparisons.len(), 1, "{:?}", report.comparisons);
    let c = &report.comparisons[0];
    assert_eq!(c.treatment.arm, "machine");
    assert_eq!(c.baseline.arm, "hold");

    // Paired at period 2: hold {0:20, 1:24, 2:16}, machine {0:22, 1:27, 2:20}.
    // Differences 2, 3, 4 — mean 3.
    assert_eq!(c.n_pairs, 3, "replicates 0, 1 and 2 are the paired subset");
    assert_eq!(c.mean_difference, Some(3.0));
    assert_eq!(c.baseline.mean, Some(20.0));
    assert_eq!(c.treatment.mean, Some(23.0));
    // hold #3 has no machine twin; machine #4 has no hold twin.
    assert_eq!(c.dropped_unpaired, 2);
    assert_eq!(c.dropped_nonfinite, 0);
    let test = c
        .test
        .as_ref()
        .expect("three pairs support a paired t-test");
    assert_eq!(test.dof, 2);
    assert!(
        test.p_value > 0.0 && test.p_value < 0.1,
        "p = {}",
        test.p_value
    );

    // The run that recorded nothing is visible as a drop, not as a silent
    // absence.
    let machine = report
        .arms
        .iter()
        .find(|a| a.arm == "machine")
        .expect("machine coverage");
    assert_eq!(machine.replicates.len(), 5);
    assert_eq!(machine.scored, 4, "#3 recorded no period to score");
    assert_eq!(machine.short, 1);
    let dead = machine
        .replicates
        .iter()
        .find(|r| r.replicate == 3)
        .expect("#3");
    assert_eq!(dead.reach, 0);
    assert!(!dead.scored);

    let hold = report
        .arms
        .iter()
        .find(|a| a.arm == "hold")
        .expect("hold coverage");
    assert_eq!(hold.scored, 4, "every hold draw reached period 2");
    assert_eq!(hold.short, 0);
}

/// Pinning a horizon is what rescues a race one short run would otherwise drag
/// to period 2 — and the replicates that cannot reach it are dropped and
/// counted, never truncated to their own last row.
#[tokio::test]
async fn a_pinned_horizon_drops_the_replicates_that_never_reached_it() {
    let db = setup_db().await;
    let (mine, _) = seed_workspace(&db, "race-pinned").await;

    seed_race_run(
        &db,
        mine,
        "w",
        "hold",
        0,
        &[(1, 10.0), (2, 20.0), (3, 30.0)],
    )
    .await;
    seed_race_run(
        &db,
        mine,
        "w",
        "hold",
        1,
        &[(1, 12.0), (2, 24.0), (3, 36.0)],
    )
    .await;
    seed_race_run(&db, mine, "w", "hold", 2, &[(1, 5.0), (2, 9.0)]).await;
    seed_race_run(
        &db,
        mine,
        "w",
        "machine",
        0,
        &[(1, 11.0), (2, 22.0), (3, 41.0)],
    )
    .await;
    seed_race_run(
        &db,
        mine,
        "w",
        "machine",
        1,
        &[(1, 13.0), (2, 27.0), (3, 49.0)],
    )
    .await;
    seed_race_run(&db, mine, "w", "machine", 2, &[(1, 6.0), (2, 11.0)]).await;

    let report = oxy_app::server::api::simulation::profit_race_report(
        mine,
        "w",
        oxy_app::server::api::simulation::RaceOptions {
            horizon: Some(3),
            ..Default::default()
        },
    )
    .await
    .expect("race");

    assert_eq!(report.horizon, Some(3));
    assert!(report.horizon_pinned);
    let c = &report.comparisons[0];
    assert_eq!(c.n_pairs, 2, "replicate 2 never reached period 3");
    assert_eq!(c.mean_difference, Some(12.0));
    // Replicate 2 is missing from BOTH arms at this horizon, so it is dropped
    // twice — once per arm — exactly as `race::compare` counts it.
    assert_eq!(c.dropped_unpaired, 0, "neither arm has a lone replicate 2");
    let hold = report.arms.iter().find(|a| a.arm == "hold").expect("hold");
    assert_eq!(hold.scored, 2);
    assert_eq!(hold.short, 1);
}

/// A world raced with one arm has nothing to compare it against. That is an
/// answer — the arm and its coverage — not a 500 and not an invented rival.
#[tokio::test]
async fn one_arm_races_against_nothing_and_says_so() {
    let db = setup_db().await;
    let (mine, _) = seed_workspace(&db, "race-one-arm").await;
    seed_race_run(&db, mine, "w", "machine", 0, &[(1, 10.0), (2, 20.0)]).await;
    seed_race_run(&db, mine, "w", "machine", 1, &[(1, 11.0), (2, 21.0)]).await;

    let report = oxy_app::server::api::simulation::profit_race_report(
        mine,
        "w",
        oxy_app::server::api::simulation::RaceOptions::default(),
    )
    .await
    .expect("race");
    assert_eq!(report.baseline.as_deref(), Some("machine"));
    assert!(report.comparisons.is_empty());
    assert_eq!(report.arms.len(), 1);
    assert_eq!(report.horizon, Some(2));
}

/// Two arms that share no world. The margin is not reported at all — there is
/// no pair to take a difference over — and the reason is named.
///
/// The reason used to be `no_pairs`; it is `disjoint_worlds` now, which is a
/// sharpening rather than a weakening. `no_pairs` covers two situations a
/// reader acts on differently, and this is the one where waiting will not help:
/// both arms scored, they simply drew different worlds and never will be
/// comparable. `no_pairs` is still what an arm with nothing scored answers —
/// see `runs_with_no_periods_at_all_leave_the_race_with_no_horizon`.
#[tokio::test]
async fn arms_with_no_shared_world_report_disjoint_worlds_rather_than_a_margin() {
    let db = setup_db().await;
    let (mine, _) = seed_workspace(&db, "race-disjoint").await;
    // `seed_race_run` puts replicate `k` on seed `7 + k`, so these two draws are
    // different worlds — which is exactly what a differing replicate index used
    // to be a proxy for, and now is not a proxy for at all.
    seed_race_run(&db, mine, "w", "hold", 0, &[(1, 10.0)]).await;
    seed_race_run(&db, mine, "w", "machine", 1, &[(1, 99.0)]).await;

    let report = oxy_app::server::api::simulation::profit_race_report(
        mine,
        "w",
        oxy_app::server::api::simulation::RaceOptions::default(),
    )
    .await
    .expect("race");
    let c = &report.comparisons[0];
    assert_eq!(c.n_pairs, 0);
    assert_eq!(c.mean_difference, None);
    assert!(c.test.is_none());
    assert_eq!(c.withheld.as_deref(), Some("disjoint_worlds"));
    assert_eq!(c.dropped_unpaired, 2);
}

/// One shared world. The difference is real and is reported; the p-value is
/// not, because a single draw has no sampling distribution behind it.
#[tokio::test]
async fn a_single_shared_replicate_reports_the_difference_with_no_p_value() {
    let db = setup_db().await;
    let (mine, _) = seed_workspace(&db, "race-single").await;
    seed_race_run(&db, mine, "w", "hold", 0, &[(1, 10.0), (2, 20.0)]).await;
    seed_race_run(&db, mine, "w", "machine", 0, &[(1, 11.0), (2, 26.0)]).await;

    let report = oxy_app::server::api::simulation::profit_race_report(
        mine,
        "w",
        oxy_app::server::api::simulation::RaceOptions::default(),
    )
    .await
    .expect("race");
    let c = &report.comparisons[0];
    assert_eq!(c.n_pairs, 1);
    assert_eq!(c.mean_difference, Some(6.0));
    assert!(c.test.is_none(), "one draw cannot support a t-test");
    assert_eq!(c.withheld.as_deref(), Some("single_pair"));
}

/// Every run died before its first period. There is no horizon to race at, and
/// the response says so rather than picking one.
#[tokio::test]
async fn runs_with_no_periods_at_all_leave_the_race_with_no_horizon() {
    let db = setup_db().await;
    let (mine, _) = seed_workspace(&db, "race-empty").await;
    seed_race_run(&db, mine, "w", "hold", 0, &[]).await;
    seed_race_run(&db, mine, "w", "machine", 0, &[]).await;

    let report = oxy_app::server::api::simulation::profit_race_report(
        mine,
        "w",
        oxy_app::server::api::simulation::RaceOptions::default(),
    )
    .await
    .expect("a run with no periods is a fact, not a failure");
    assert_eq!(report.horizon, None);
    let c = &report.comparisons[0];
    assert_eq!(c.n_pairs, 0);
    assert_eq!(c.withheld.as_deref(), Some("no_pairs"));
    for arm in &report.arms {
        assert_eq!(arm.scored, 0);
        assert_eq!(arm.short, 1);
    }
}

/// A world nobody has run is an empty race, not a 404 — the world may well
/// exist and simply have no runs yet.
#[tokio::test]
async fn a_world_with_no_runs_is_an_empty_race() {
    let db = setup_db().await;
    let (mine, _) = seed_workspace(&db, "race-none").await;
    let report = oxy_app::server::api::simulation::profit_race_report(
        mine,
        "w",
        oxy_app::server::api::simulation::RaceOptions::default(),
    )
    .await
    .expect("race");
    assert!(report.arms.is_empty());
    assert!(report.comparisons.is_empty());
    assert_eq!(report.baseline, None);
    assert_eq!(report.horizon, None);
}

// ── the pairing key, and what may be scored ──────────────────────────────────
//
// Two failures that a replicate-keyed, status-blind race produces silently.
// Both come out as a number, and nothing in the number says it is wrong, which
// is why they are pinned end to end against real rows rather than left to the
// pure tests alone.

/// The pairing premise is that replicate *k* of every arm saw the same world.
/// That holds only while the spec's `seed` is unchanged — `replicate_seed(base,
/// k) = base + k` — so an edit to `seed:` between queueing two arms makes
/// replicate 0 of each a *different* world. Pairing them anyway is not a weaker
/// comparison, it is a fabricated one: the difference is world-to-world
/// variance, which is the largest term in the exercise and the one pairing
/// exists to remove.
#[tokio::test]
async fn runs_queued_under_different_seeds_are_not_paired_as_one_world() {
    let db = setup_db().await;
    let (mine, _) = seed_workspace(&db, "race-seed-drift").await;

    seed_race_run_as(
        &db,
        mine,
        RaceRunSeed {
            policy: "hold",
            replicate: 0,
            seed: 7,
            periods: &[(1, 10.0), (2, 20.0)],
            ..Default::default()
        },
    )
    .await;
    // Same replicate index, a different world — someone retuned `seed:` between
    // the two queueings.
    seed_race_run_as(
        &db,
        mine,
        RaceRunSeed {
            policy: "machine",
            replicate: 0,
            seed: 99,
            periods: &[(1, 11.0), (2, 26.0)],
            ..Default::default()
        },
    )
    .await;

    let report = oxy_app::server::api::simulation::profit_race_report(
        mine,
        "w",
        oxy_app::server::api::simulation::RaceOptions::default(),
    )
    .await
    .expect("race");

    let c = &report.comparisons[0];
    assert_eq!(
        c.n_pairs, 0,
        "replicate 0 of the two arms drew different worlds"
    );
    assert_eq!(
        c.mean_difference, None,
        "6.0 here would be world variance sold as a policy effect"
    );
    assert_eq!(
        c.withheld.as_deref(),
        Some("disjoint_worlds"),
        "both arms scored, they just never shared a world — that is not `no_pairs`"
    );
    assert_eq!(c.dropped_unpaired, 2);
    // The seed is on the wire, because `replicate` alone no longer identifies a
    // draw once a base seed has moved.
    let machine = report
        .arms
        .iter()
        .find(|a| a.arm == "machine")
        .expect("machine coverage");
    assert_eq!(machine.replicates[0].replicate, 0);
    assert_eq!(machine.replicates[0].seed, 99);
}

/// A re-queued run is `queued` with no period rows, and it is the newest row
/// for its draw. Letting it win deduplication evicts the completed run it was
/// meant to repeat, whose curve then reads as empty — so re-running a world
/// blanked the finished race that was already there.
#[tokio::test]
async fn a_requeued_run_does_not_evict_the_completed_run_it_supersedes() {
    let db = setup_db().await;
    let (mine, _) = seed_workspace(&db, "race-requeued").await;

    seed_race_run_as(
        &db,
        mine,
        RaceRunSeed {
            policy: "hold",
            seed: 7,
            periods: &[(1, 10.0), (2, 20.0)],
            ..Default::default()
        },
    )
    .await;
    seed_race_run_as(
        &db,
        mine,
        RaceRunSeed {
            policy: "machine",
            seed: 7,
            periods: &[(1, 11.0), (2, 26.0)],
            ..Default::default()
        },
    )
    .await;
    // Someone hit "run again" on the machine arm a minute later.
    seed_race_run_as(
        &db,
        mine,
        RaceRunSeed {
            policy: "machine",
            seed: 7,
            status: "queued",
            queued_offset_secs: 60,
            periods: &[],
            ..Default::default()
        },
    )
    .await;

    let report = oxy_app::server::api::simulation::profit_race_report(
        mine,
        "w",
        oxy_app::server::api::simulation::RaceOptions::default(),
    )
    .await
    .expect("race");

    assert_eq!(report.horizon, Some(2));
    let c = &report.comparisons[0];
    assert_eq!(c.n_pairs, 1, "the finished race is still there");
    assert_eq!(c.mean_difference, Some(6.0));

    let machine = report
        .arms
        .iter()
        .find(|a| a.arm == "machine")
        .expect("machine coverage");
    assert_eq!(
        machine.replicates.len(),
        1,
        "one world, whichever run of it is readable"
    );
    assert_eq!(machine.scored, 1);
    assert_eq!(machine.short, 0);
    // Visible the way `superseded_runs` is, and NOT counted as one: a run that
    // has not finished has superseded nothing.
    assert_eq!(report.in_flight_runs, 1);
    assert_eq!(report.superseded_runs, 0);
}

/// Neither arm has a world the other drew. That is a specific, nameable
/// outcome — the two arms were run against different worlds — and it must not
/// come back as the same `no_pairs` an unrun world produces, which a reader
/// glosses as "no difference".
#[tokio::test]
async fn arms_with_disjoint_seed_sets_name_the_reason_rather_than_reading_as_no_difference() {
    let db = setup_db().await;
    let (mine, _) = seed_workspace(&db, "race-disjoint-seeds").await;

    for (replicate, seed, curve) in [
        (0, 7_i64, [(1, 10.0), (2, 20.0)]),
        (1, 8, [(1, 12.0), (2, 24.0)]),
    ] {
        seed_race_run_as(
            &db,
            mine,
            RaceRunSeed {
                policy: "hold",
                replicate,
                seed,
                periods: &curve,
                ..Default::default()
            },
        )
        .await;
    }
    for (replicate, seed, curve) in [
        (0, 100_i64, [(1, 11.0), (2, 30.0)]),
        (1, 101, [(1, 13.0), (2, 34.0)]),
    ] {
        seed_race_run_as(
            &db,
            mine,
            RaceRunSeed {
                policy: "machine",
                replicate,
                seed,
                periods: &curve,
                ..Default::default()
            },
        )
        .await;
    }

    let report = oxy_app::server::api::simulation::profit_race_report(
        mine,
        "w",
        oxy_app::server::api::simulation::RaceOptions::default(),
    )
    .await
    .expect("race");

    assert_eq!(report.horizon, Some(2));
    let c = &report.comparisons[0];
    assert_eq!(c.n_pairs, 0);
    assert_eq!(c.mean_difference, None);
    assert!(c.test.is_none());
    assert_eq!(c.withheld.as_deref(), Some("disjoint_worlds"));
    assert_eq!(c.dropped_unpaired, 4, "two worlds each, none shared");
    // Coverage still reads sensibly: both arms ran and both reached the
    // horizon. Nothing here is short — they simply do not overlap.
    for arm in &report.arms {
        assert_eq!(arm.scored, 2, "{} coverage", arm.arm);
        assert_eq!(arm.short, 0, "{} coverage", arm.arm);
    }
}

/// A `running` run's curve grows between two identical requests, so scoring it
/// would move the horizon — and every margin under it — with nothing in the
/// response saying why. It is excluded and counted, and it does not displace
/// the finished run of the same world.
#[tokio::test]
async fn a_running_run_is_excluded_and_counted_rather_than_scored() {
    let db = setup_db().await;
    let (mine, _) = seed_workspace(&db, "race-running").await;

    seed_race_run_as(
        &db,
        mine,
        RaceRunSeed {
            policy: "hold",
            seed: 7,
            periods: &[(1, 10.0), (2, 20.0), (3, 30.0)],
            ..Default::default()
        },
    )
    .await;
    seed_race_run_as(
        &db,
        mine,
        RaceRunSeed {
            policy: "machine",
            seed: 7,
            periods: &[(1, 11.0), (2, 22.0), (3, 36.0)],
            ..Default::default()
        },
    )
    .await;
    // In flight, one period in, and newer than the run it repeats.
    seed_race_run_as(
        &db,
        mine,
        RaceRunSeed {
            policy: "machine",
            seed: 7,
            status: "running",
            queued_offset_secs: 60,
            periods: &[(1, 1.0)],
            ..Default::default()
        },
    )
    .await;

    let report = oxy_app::server::api::simulation::profit_race_report(
        mine,
        "w",
        oxy_app::server::api::simulation::RaceOptions::default(),
    )
    .await
    .expect("race");

    assert_eq!(
        report.horizon,
        Some(3),
        "a partial run must not drag the horizon to its own last row"
    );
    let c = &report.comparisons[0];
    assert_eq!(c.n_pairs, 1);
    assert_eq!(c.mean_difference, Some(6.0));
    assert_eq!(report.in_flight_runs, 1);

    let machine = report
        .arms
        .iter()
        .find(|a| a.arm == "machine")
        .expect("machine coverage");
    assert_eq!(machine.replicates.len(), 1);
    assert_eq!(
        machine.replicates[0].reach, 3,
        "the finished curve, not the in-flight one"
    );
}

/// Pairing on the seed is strictly more correct than pairing on the replicate,
/// not merely safer. With the base seed edited from 7 to 8 between two arms,
/// `hold` #1 and `machine` #0 are both seed 8 — the same world — and they pair
/// even though their replicate numbers differ. A replicate-keyed race pairs the
/// two arms twice and gets both pairs wrong.
#[tokio::test]
async fn a_shifted_base_seed_pairs_the_replicates_that_share_a_world() {
    let db = setup_db().await;
    let (mine, _) = seed_workspace(&db, "race-shifted-base").await;

    // `hold` fanned out from base 7.
    for (replicate, seed, curve) in [
        (0, 7_i64, [(1, 10.0), (2, 20.0)]),
        (1, 8, [(1, 12.0), (2, 24.0)]),
    ] {
        seed_race_run_as(
            &db,
            mine,
            RaceRunSeed {
                policy: "hold",
                replicate,
                seed,
                periods: &curve,
                ..Default::default()
            },
        )
        .await;
    }
    // …then `seed:` became 8, and `machine` fanned out from there.
    for (replicate, seed, curve) in [
        (0, 8_i64, [(1, 13.0), (2, 30.0)]),
        (1, 9, [(1, 5.0), (2, 9.0)]),
    ] {
        seed_race_run_as(
            &db,
            mine,
            RaceRunSeed {
                policy: "machine",
                replicate,
                seed,
                periods: &curve,
                ..Default::default()
            },
        )
        .await;
    }

    let report = oxy_app::server::api::simulation::profit_race_report(
        mine,
        "w",
        oxy_app::server::api::simulation::RaceOptions::default(),
    )
    .await
    .expect("race");

    let c = &report.comparisons[0];
    assert_eq!(
        c.n_pairs, 1,
        "seed 8 is the one world both arms drew — hold #1 against machine #0"
    );
    // 30 − 24. NOT the replicate-keyed (20 vs 30) and (24 vs 9), whose mean of
    // −2.5 is a number about two different worlds.
    assert_eq!(c.mean_difference, Some(6.0));
    assert_eq!(c.withheld.as_deref(), Some("single_pair"));
    assert_eq!(c.dropped_unpaired, 2, "seed 7 and seed 9 have no twin");

    // The label a reader recognises is still the replicate; the seed is what
    // says which of them are the same world.
    let hold = report.arms.iter().find(|a| a.arm == "hold").expect("hold");
    assert_eq!(
        hold.replicates
            .iter()
            .map(|r| (r.replicate, r.seed))
            .collect::<Vec<_>>(),
        vec![(0, 7), (1, 8)]
    );
}
