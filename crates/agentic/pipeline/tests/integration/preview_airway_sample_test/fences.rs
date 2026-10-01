//! The fences around a sample: the store it may use, the platform it may run
//! on, the step hold, and QuickBooks' credentials.

use std::collections::HashSet;

use agentic_airway::extension::pipeline_lease;
use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_pipeline::PREVIEW_AIRWAY_SAMPLE;
use agentic_pipeline::airway_preview::{PreviewSample, SampleWindow, SandboxSource};
use agentic_runtime::worker::TaskExecutor;
use chrono::TimeZone;
use uuid::Uuid;

use super::fixture::{
    REF, SamplePlatform, audit_rows, drive, executor, preview_name, row_text, sample, scope,
    seed_run, state_rows, users,
};
use super::{is_done, toast_stub, unique};
use crate::airway_run_test::test_db;

/// A windowed sample of a source whose production backfill resumes through the
/// run-scoped store (Toast) still takes the pipeline-global store, under its
/// preview name: its schema is stored there and no cursor lands on the run.
#[tokio::test(flavor = "multi_thread")]
async fn sample_never_uses_the_run_scoped_store() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    assert!(
        agentic_pipeline::executor::RESUMABLE_BACKFILL_KINDS.contains(&"toast"),
        "precondition: production resumes a Toast window through the run-scoped store"
    );
    let base = toast_stub::serve().await;
    let name = unique("it_sample_toast");
    let yaml = format!(
        "name: {name}
source:
  kind: toast
  config:
    client_id: stub-client
    client_secret_var: TOAST_STUB_SECRET
    restaurant_guids: [r-1]
    base_url: {base}
destination:
  kind: memory
  config:
    dataset_name: scratch
resources: [orders]
"
    );
    let ws = Uuid::new_v4();
    let run_id = Uuid::new_v4().to_string();
    seed_run(&db, &run_id, PREVIEW_AIRWAY_SAMPLE, ws).await;
    let mut platform = SamplePlatform::new(ws, Some(scope(&run_id)), yaml);
    platform
        .secrets
        .insert("TOAST_STUB_SECRET".into(), "s".into());
    let window = SampleWindow {
        from: chrono::Utc.with_ymd_and_hms(2026, 9, 20, 0, 0, 0).unwrap(),
        to: chrono::Utc.with_ymd_and_hms(2026, 9, 23, 0, 0, 0).unwrap(),
    };
    let toast_sample = PreviewSample {
        window: Some(window),
        resources: vec![],
        ..sample()
    };
    let task = executor(&db, platform)
        .execute_airway_preview_sample(&run_id, REF, &toast_sample)
        .await
        .expect("dispatch");
    let (outcome, _) = drive(task).await;
    assert!(is_done(&outcome), "{outcome:?}");

    let state = state_rows(&db, ws, &preview_name(&name)).await;
    assert_eq!(
        state.len(),
        1,
        "the pipeline-global store saved under the preview name"
    );
    assert!(
        state[0].contains("\"orders\""),
        "its schema was stored: {state:?}"
    );
    let resume = row_text(
        &db,
        "SELECT COALESCE(resume_state::text, 'null') AS t FROM airway_run_extensions WHERE run_id = $1",
        vec![run_id.into()],
    )
    .await;
    assert_eq!(resume, vec!["null".to_string()], "no cursor on the run");
    assert!(state_rows(&db, ws, &name).await.is_empty());
}

/// An `airway` step inside a preview procedure run is held before anything is
/// leased or loaded, against a real database: no lease, no state, no audit
/// under either name.
#[tokio::test(flavor = "multi_thread")]
async fn airway_step_in_a_preview_run_holds_instead_of_leasing() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let name = unique("it_step_held");
    let (_dir, yaml) = users(&name);
    let root = Uuid::new_v4().to_string();
    seed_run(&db, &root, "workflow", ws).await;
    let child = format!("{root}.1");
    let exec = executor(&db, SamplePlatform::new(ws, Some(scope(&root)), yaml));
    let task = exec
        .execute(TaskAssignment {
            task_id: child.clone(),
            parent_task_id: Some(root.clone()),
            run_id: child,
            spec: TaskSpec::Airway {
                pipeline_ref: agentic_automation::preview_names::scoped(&root, REF),
                variables: None,
                resources: vec![],
                backfill_from: None,
                backfill_to: None,
                contract_policy: None,
                environment: None,
            },
            policy: None,
        })
        .await
        .expect("a held step is not an error");
    let TaskOutcome::Done { metadata, .. } = drive(task).await.0 else {
        panic!("held steps are Done");
    };
    assert_eq!(metadata.unwrap()["preview"]["held"], true);
    let leases = pipeline_lease::list_for_workspace(&db, ws).await.unwrap();
    assert!(leases.is_empty(), "{leases:?}");
    for n in [name.clone(), preview_name(&name)] {
        assert!(state_rows(&db, ws, &n).await.is_empty(), "{n}");
        assert!(audit_rows(&db, ws, &n).await.is_empty(), "{n}");
    }
}

fn quickbooks_yaml(name: &str) -> String {
    format!(
        "name: {name}
source:
  kind: quickbooks
  config:
    client_id: PROD_CLIENT
    client_secret_var: QB_PROD_CLIENT_SECRET
    refresh_token_var: QB_PROD_REFRESH_TOKEN
    realm_id: 9341456860808037
destination:
  database: airhouse
  dataset_name: quickbooks_eastbay
"
    )
}

/// A QuickBooks sample asks the platform only for the sandbox's vars, and
/// fails (releasing its lease) at the destination this platform will not give
/// — before any request to Intuit. The platform panics on a persist of any
/// var but the sandbox's rotating one.
#[tokio::test(flavor = "multi_thread")]
async fn quickbooks_sample_never_requests_a_production_var() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let name = unique("it_sample_qb");
    let run_id = Uuid::new_v4().to_string();
    seed_run(&db, &run_id, PREVIEW_AIRWAY_SAMPLE, ws).await;
    let sandbox = SandboxSource {
        realm_id: "4620816365000000".into(),
        refresh_token_var: Some("QB_SANDBOX_REFRESH".into()),
        access_token_var: None,
        client_id: None,
        client_id_var: Some("QB_SANDBOX_CLIENT_ID".into()),
        client_secret_var: Some("QB_SANDBOX_SECRET".into()),
    };
    let mut platform = SamplePlatform::new(ws, Some(scope(&run_id)), quickbooks_yaml(&name));
    for var in sandbox.var_names() {
        platform
            .secrets
            .insert(var.to_string(), "sandbox-value".into());
    }
    platform.rotating = sandbox.rotating_var().map(str::to_string);
    let platform = std::sync::Arc::new(platform);
    let exec = agentic_pipeline::executor::PipelineTaskExecutor::bare(platform.clone(), db.clone());
    let to = chrono::Utc.with_ymd_and_hms(2026, 9, 27, 0, 0, 0).unwrap();
    let qb = PreviewSample {
        sandbox: Some(sandbox.clone()),
        window: Some(SampleWindow {
            from: to - chrono::Duration::days(7),
            to,
        }),
        resources: vec![],
        ..sample()
    };
    let err = match exec.execute_airway_preview_sample(&run_id, REF, &qb).await {
        Ok(_) => panic!("this platform resolves no destination"),
        Err(e) => e,
    };
    assert!(err.contains("destination"), "{err}");

    let asked: HashSet<String> = platform.asked.lock().unwrap().iter().cloned().collect();
    let sandbox_vars: HashSet<String> = sandbox.var_names().iter().map(|v| v.to_string()).collect();
    assert_eq!(
        asked, sandbox_vars,
        "exactly the sandbox's vars were asked for"
    );
    for production in ["QB_PROD_CLIENT_SECRET", "QB_PROD_REFRESH_TOKEN"] {
        assert!(!asked.contains(production), "{production} was requested");
    }
    let leases = pipeline_lease::list_for_workspace(&db, ws).await.unwrap();
    assert!(
        leases.is_empty(),
        "a failed dispatch released its lease: {leases:?}"
    );
}

/// On a platform that is not a preview's, or not this run's, a sample does not
/// start: its destination and secrets would be production's.
#[tokio::test(flavor = "multi_thread")]
async fn a_sample_runs_only_on_its_own_preview_platform() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let (_dir, yaml) = users(&unique("it_sample_platform"));
    let run_id = Uuid::new_v4().to_string();
    for platform_scope in [None, Some(scope("another-run"))] {
        let platform = SamplePlatform::new(ws, platform_scope, yaml.clone());
        let refused = executor(&db, platform)
            .execute_airway_preview_sample(&run_id, REF, &sample())
            .await;
        assert!(refused.is_err(), "refused");
    }
    assert!(
        pipeline_lease::list_for_workspace(&db, ws)
            .await
            .unwrap()
            .is_empty()
    );
}
