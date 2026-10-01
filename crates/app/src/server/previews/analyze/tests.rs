//! The change check's Postgres halves against a real database: which pipelines
//! a branch changed, and one check per revision however many callers ask.
//! Skips (does not fail) when `OXY_DATABASE_URL` is unset, per
//! `test_support::test_db`; every assertion is scoped to the workspace its test
//! seeded, so the tests share the database without a lock.

use entity::{airway_pipelines, revisions, workspaces};
use sea_orm::{
    ActiveModelTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, Set,
    Statement,
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::*;
use crate::server::test_support::{SKIP_MSG, test_db};

pub(crate) async fn seed_workspace(db: &DatabaseConnection) -> Uuid {
    let id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: Set(id),
        name: Set(format!("previews-analyze-{id}")),
        status: Set(workspaces::WorkspaceStatus::Ready),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed workspace");
    id
}

pub(crate) async fn seed_revision(db: &DatabaseConnection, ws: Uuid, kind: &str) -> Uuid {
    let id = Uuid::new_v4();
    seed_revision_at(db, ws, kind, id, &format!("sha-{id}")).await
}

/// A ready revision `id` of `git_sha`, reusable by this build
/// (`oxy_compile::find_reusable_revision`).
pub(crate) async fn seed_revision_at(
    db: &DatabaseConnection,
    ws: Uuid,
    kind: &str,
    id: Uuid,
    git_sha: &str,
) -> Uuid {
    let now = chrono::Utc::now().fixed_offset();
    revisions::ActiveModel {
        revision_id: Set(id),
        workspace_id: Set(ws),
        git_sha: Set(git_sha.into()),
        branch: Set(Some(if kind == "main" { "main" } else { "feat/x" }.into())),
        schema_version: Set(oxy_compile::CURRENT_SCHEMA_VERSION),
        status: Set("ready".into()),
        kind: Set(kind.into()),
        owner_user_id: Set(None),
        compiler_version: Set(oxy_compile::compiler_version()),
        started_at: Set(now),
        finished_at: Set(Some(now)),
        file_count_seen: Set(0),
        file_count_compiled: Set(0),
        file_count_failed: Set(0),
        error_summary: Set(None),
    }
    .insert(db)
    .await
    .expect("seed revision");
    id
}

async fn seed_pipeline(db: &DatabaseConnection, rev: Uuid, name: &str, path: &str, def: Value) {
    airway_pipelines::ActiveModel {
        revision_id: Set(rev),
        name: Set(name.into()),
        file_path: Set(path.into()),
        definition: Set(def),
    }
    .insert(db)
    .await
    .expect("seed pipeline");
}

fn def(name: &str, dataset: &str) -> Value {
    json!({
        "name": name,
        "source": { "kind": "toast", "config": { "client_id": "c" } },
        "destination": { "database": "warehouse", "dataset_name": dataset },
    })
}

#[tokio::test]
async fn changed_pipelines_query_finds_edits_adds_and_removals() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let ws = seed_workspace(&db).await;
    let main = seed_revision(&db, ws, "main").await;
    let staging = seed_revision(&db, ws, "staging").await;
    // Unchanged, edited, removed on main; the same unchanged and edited files,
    // plus one new file, on the branch.
    seed_pipeline(
        &db,
        main,
        "same",
        "airway/a_same.airway.yml",
        def("same", "raw"),
    )
    .await;
    seed_pipeline(
        &db,
        main,
        "edited",
        "airway/b_edited.airway.yml",
        def("edited", "raw"),
    )
    .await;
    seed_pipeline(
        &db,
        main,
        "gone",
        "airway/c_gone.airway.yml",
        def("gone", "raw"),
    )
    .await;
    seed_pipeline(
        &db,
        staging,
        "same",
        "airway/a_same.airway.yml",
        def("same", "raw"),
    )
    .await;
    seed_pipeline(
        &db,
        staging,
        "edited",
        "airway/b_edited.airway.yml",
        def("edited", "raw_v2"),
    )
    .await;
    seed_pipeline(
        &db,
        staging,
        "new",
        "airway/d_new.airway.yml",
        def("new", "raw"),
    )
    .await;

    let changed = changed_pipelines(&db, staging, Some(main)).await.unwrap();
    let got: Vec<(&str, &str, Change)> = changed
        .iter()
        .map(|c| (c.name.as_str(), c.file_path.as_str(), c.change()))
        .collect();
    assert_eq!(
        got,
        vec![
            ("edited", "airway/b_edited.airway.yml", Change::Modified),
            ("gone", "airway/c_gone.airway.yml", Change::Removed),
            ("new", "airway/d_new.airway.yml", Change::Added),
        ],
        "an identical file is not a change; edits, removals and additions are"
    );
    let edited = &changed[0];
    assert_eq!(
        edited.live_def.as_ref().unwrap()["destination"]["dataset_name"],
        "raw"
    );
    assert_eq!(
        edited.branch_def.as_ref().unwrap()["destination"]["dataset_name"],
        "raw_v2"
    );
    assert!(changed[1].branch_def.is_none() && changed[1].live_def.is_some());
    assert!(changed[2].live_def.is_none() && changed[2].branch_def.is_some());

    // No promoted revision yet: everything on the branch is an addition.
    let fresh = changed_pipelines(&db, staging, None).await.unwrap();
    assert_eq!(fresh.len(), 3);
    assert!(fresh.iter().all(|c| c.change() == Change::Added));

    // A revision compared with itself changed nothing.
    assert!(
        changed_pipelines(&db, main, Some(main))
            .await
            .unwrap()
            .is_empty()
    );
}

async fn count(db: &DatabaseConnection, sql: &str, run_id: &str) -> i64 {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            [run_id.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    row.try_get::<i64>("", "n").unwrap()
}

#[tokio::test]
async fn ensure_enqueued_is_idempotent_per_revision() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let ws = seed_workspace(&db).await;
    let rev = seed_revision(&db, ws, "staging").await;

    // Three callers race for the same revision — the compile worker and two
    // refreshes, say. Exactly one wins.
    let (a, b, c) = tokio::join!(
        ensure_enqueued(&db, ws, "feat/x", rev),
        ensure_enqueued(&db, ws, "feat/x", rev),
        ensure_enqueued(&db, ws, "feat/x", rev),
    );
    let winners: Vec<String> = [a, b, c].into_iter().filter_map(|r| r.unwrap()).collect();
    assert_eq!(winners.len(), 1, "one check per revision: {winners:?}");
    assert_eq!(ensure_enqueued(&db, ws, "feat/x", rev).await.unwrap(), None);

    let rows = entity::workspace_preview_runs::Entity::find()
        .all(&db)
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.workspace_id == ws)
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.run_id, winners[0]);
    assert_eq!(
        (row.kind.as_str(), row.state.as_str()),
        ("analyze", "queued")
    );
    assert_eq!(row.revision_id, rev);
    assert_eq!(
        row.preview_key,
        crate::server::previews::namespace::preview_key(ws, "feat/x")
    );

    // The winner seeded its run and queued its task, globally scoped, in the
    // same transaction as the row.
    assert_eq!(
        count(
            &db,
            "SELECT count(*) AS n FROM agentic_runs WHERE id = $1 AND source_type = 'preview_analyze'",
            &row.run_id,
        )
        .await,
        1
    );
    let task = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT spec, scope_owned FROM agentic_task_queue WHERE run_id = $1",
            [row.run_id.clone().into()],
        ))
        .await
        .unwrap()
        .expect("the check's task is queued");
    let spec: Value = task.try_get("", "spec").unwrap();
    assert_eq!(spec["kind"], PREVIEW_ANALYZE_KIND);
    assert_eq!(spec["payload"]["preview_run_id"], row.run_id.as_str());
    assert!(
        !task.try_get::<bool>("", "scope_owned").unwrap(),
        "TaskScope::Global"
    );

    // Another revision is another check.
    let next = seed_revision(&db, ws, "staging").await;
    assert!(
        ensure_enqueued(&db, ws, "feat/x", next)
            .await
            .unwrap()
            .is_some()
    );
}

#[test]
fn the_executor_refuses_a_spec_that_is_not_its_own() {
    let wrong = TaskSpec::Custom {
        kind: "preagg_cycle".into(),
        payload: json!({ "preview_run_id": "r" }),
    };
    assert!(preview_run_id(&wrong).unwrap_err().contains("preagg_cycle"));
    let missing = TaskSpec::Custom {
        kind: PREVIEW_ANALYZE_KIND.into(),
        payload: json!({}),
    };
    assert!(
        preview_run_id(&missing)
            .unwrap_err()
            .contains("preview_run_id")
    );
    let ok = TaskSpec::Custom {
        kind: PREVIEW_ANALYZE_KIND.into(),
        payload: json!({ "preview_run_id": "r-1" }),
    };
    assert_eq!(preview_run_id(&ok).unwrap(), "r-1");
}

/// P2 over definitions shaped like pokehouse's (synthetic SQL; the shapes
/// follow the plan's walk-through): of its six Airhouse transforms, the five
/// that need no value build on their own, and the journal entry — whose SQL
/// names `{{ date }}`, which a build (run with no variables) cannot give — is
/// manual; so is a procedure mixing in anything else — an airway step, an
/// agent, a ClickHouse write — with that reason.
#[test]
fn pure_airhouse_procedures_auto_build_and_mixed_ones_are_manual() {
    use super::transform_tests::{databases, pokehouse, sql_file};
    let dbs = databases();
    let auto = [
        "rollups/restaurant_analytics_daily_rollups",
        "rollups/camera_time_to_serve_rollups",
        "site_selection_refresh",
        "qb_je_account_map_seed_airhouse",
        "bookkeeping_provision_airhouse",
    ];
    let manual = [
        (
            "compute_toast_journal_entry_airhouse",
            "needs variables: date ",
        ),
        ("toast_ingest_and_rollups", "calls an airway step"),
        ("compute_toast_journal_entry", "writes ClickHouse"),
        ("restaurant_insights", "calls an agent"),
        ("weekly_report", "writes nothing"),
    ];
    for name in auto {
        assert_eq!(
            classify(&pokehouse(name), &dbs, &sql_file),
            Build::Auto,
            "{name}"
        );
    }
    for (name, why) in manual {
        match classify(&pokehouse(name), &dbs, &sql_file) {
            Build::Manual(reason) => assert!(reason.starts_with(why), "{name}: {reason}"),
            Build::Auto => panic!("{name} is not a pure-Airhouse transform"),
        }
    }
}
