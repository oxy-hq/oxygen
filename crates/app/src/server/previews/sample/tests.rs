//! The submit rules as the host applies them, the deadline, and what a cut-off
//! sample's outcome says. No database: recording against a disconnected one
//! fails, and that failure is carried in the outcome rather than failing it.

use std::time::Duration;

use agentic_airway::AirwayPipelineSpec;
use agentic_core::delegation::TaskOutcome;
use agentic_runtime::worker::ExecutingTask;
use axum::http::StatusCode;
use chrono::Utc;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::outcome::{Watched, watch};
use super::*;
use crate::server::previews::runs::RunRequestError;

fn spec(kind: &str, config: &str, destination: &str) -> AirwayPipelineSpec {
    let yaml = format!(
        "name: p\nsource:\n  kind: {kind}\n  config:\n{config}\ndestination:\n{destination}\n"
    );
    AirwayPipelineSpec::from_yaml_str(&yaml).expect("the fixture parses")
}

fn ask(
    spec: &AirwayPipelineSpec,
    resources: &[String],
    has_sandbox: bool,
) -> Result<SampleOptions, agentic_airway::preview::SampleRefusal> {
    let asked = Asked {
        window: None,
        resources,
        has_sandbox,
    };
    check(spec, &asked, None, Utc::now())
}

const AIRHOUSE: &str = "  database: airhouse\n  dataset_name: raw_p";

fn rest_api(endpoints: &[&str]) -> AirwayPipelineSpec {
    let mut config = "    base_url: https://example.test\n    endpoints:\n".to_string();
    for e in endpoints {
        config.push_str(&format!("      - name: {e}\n        path: /{e}\n"));
    }
    spec("rest_api", config.trim_end(), AIRHOUSE)
}

#[test]
fn cdc_and_sp_api_are_refused() {
    let config = "    connection_string: postgres://x/y";
    for kind in ["postgres_cdc", "pgoutput", "sp_api"] {
        let s = spec(kind, config, AIRHOUSE);
        let refusal = ask(&s, &["orders".into()], false).unwrap_err();
        assert_eq!(refusal.code(), "sample_refused", "{kind}");
        let err = RunRequestError::Sample(refusal);
        assert_eq!(err.status(), StatusCode::UNPROCESSABLE_ENTITY, "{kind}");
        assert_eq!(err.code(), "sample_refused");
    }
}

#[test]
fn non_windowed_needs_resources() {
    let two = rest_api(&["orders", "customers"]);
    let err = ask(&two, &[], false).unwrap_err();
    assert_eq!(err.code(), "resources_required", "{err}");
    assert!(
        err.to_string().contains("orders"),
        "names what it advertises: {err}"
    );
    assert_eq!(
        RunRequestError::Sample(err).status(),
        StatusCode::BAD_REQUEST
    );

    let named = ask(&two, &["orders".into()], false).unwrap();
    assert_eq!(named.resources, vec!["orders".to_string()]);
    assert!(named.wall_clock_capped, "no window bounds it");
    assert_eq!(named.dataset_name.as_deref(), Some("raw_p"));
    assert_eq!(named.window, None);

    let one = rest_api(&["orders"]);
    assert!(
        ask(&one, &[], false).is_ok(),
        "a single resource needs no naming"
    );
    let unknown = ask(&two, &["refunds".into()], false).unwrap_err();
    assert_eq!(unknown.code(), "unknown_resource");
}

#[test]
fn a_rotate_on_use_source_needs_its_sandbox_and_a_window_source_gets_a_week() {
    let qb = spec(
        "quickbooks",
        "    client_id: c\n    refresh_token_var: QB_R\n    client_secret_var: QB_S\n    realm_id: \"1\"",
        AIRHOUSE,
    );
    let err = ask(&qb, &[], false).unwrap_err();
    assert_eq!(err.code(), "sandbox_required");
    assert_eq!(RunRequestError::Sample(err).status(), StatusCode::CONFLICT);
    let ok = ask(&qb, &[], true).unwrap();
    let window = ok.window.expect("windowed");
    assert_eq!(window.to - window.from, chrono::Duration::days(7));
    assert!(!ok.wall_clock_capped);
}

#[test]
fn an_inline_destination_other_than_memory_is_refused() {
    let inline = "  kind: postgres\n  config:\n    connection_string: postgres://prod/db\n    dataset_name: raw";
    let s = spec(
        "rest_api",
        "    base_url: https://x.test\n    endpoints:\n      - name: a\n        path: /a",
        inline,
    );
    let err = ask(&s, &[], false).unwrap_err();
    assert_eq!(err.code(), "sample_refused", "{err}");
}

#[test]
fn every_sample_stops_before_the_run_ceiling() {
    let d = deadline(true, 900, 60);
    assert_eq!(d.after, Duration::from_secs(900));
    assert!(d.reason.contains("OXY_PREVIEW_SAMPLE_MAX_SECS"));
    let windowed = deadline(false, 900, 60);
    assert_eq!(
        windowed.after,
        Duration::from_secs(55 * 60),
        "5 minutes under"
    );
    assert!(windowed.reason.contains("ceiling"));
    assert_eq!(
        deadline(true, 7200, 60).after,
        Duration::from_secs(55 * 60),
        "a cap above the ceiling is the ceiling"
    );
    assert_eq!(deadline(false, 900, 1).after, Duration::from_secs(60));
}

/// A task whose engine runs until it is cancelled, and then fails as the
/// Airway worker does; `finish` makes it complete on its own instead.
fn engine(finish: Option<TaskOutcome>) -> (ExecutingTask, CancellationToken) {
    let (_events_tx, events) = mpsc::channel(4);
    let (outcomes_tx, outcomes) = mpsc::channel(4);
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    tokio::spawn(async move {
        let _keep = _events_tx;
        let outcome = match finish {
            Some(outcome) => outcome,
            None => {
                token.cancelled().await;
                TaskOutcome::Failed("airway: pipeline cancelled".into())
            }
        };
        let _ = outcomes_tx.send(outcome).await;
    });
    let task = ExecutingTask {
        events,
        outcomes,
        cancel: cancel.clone(),
        answers: None,
    };
    (task, cancel)
}

fn watched(after: Duration) -> Watched {
    Watched {
        db: sea_orm::DatabaseConnection::default(),
        workspace_id: uuid::Uuid::nil(),
        preview_key: "feat_x_abc123".into(),
        run_id: "r".into(),
        pipeline: "orders_api".into(),
        dataset: Some("raw_orders".into()),
        deadline: Deadline {
            after,
            reason: "the sample's wall-clock cap (OXY_PREVIEW_SAMPLE_MAX_SECS)",
        },
    }
}

#[tokio::test]
async fn wall_clock_cap_marks_partial() {
    let (task, cancel) = engine(None);
    let mut outer = watch(task, watched(Duration::from_millis(50)));
    let outcome = tokio::time::timeout(Duration::from_secs(10), outer.outcomes.recv())
        .await
        .expect("the cap cuts it off")
        .expect("an outcome");
    assert!(cancel.is_cancelled(), "the engine's token was cancelled");
    let TaskOutcome::Done { answer, metadata } = outcome else {
        panic!("a sample cut off at its cap is Done, partial: {outcome:?}");
    };
    let sample = &metadata.expect("metadata")["sample"];
    assert_eq!(sample["partial"], true, "{sample}");
    assert!(
        sample["partial_reason"]
            .as_str()
            .unwrap()
            .contains("OXY_PREVIEW_SAMPLE_MAX_SECS"),
        "{sample}"
    );
    assert!(answer.contains("partial"), "{answer}");

    // The controls: finishing first is not partial; a load the engine failed
    // on its own stays failed.
    let done = TaskOutcome::Done {
        answer: String::new(),
        metadata: Some(json!({ "load_id": "l" })),
    };
    let (task, _) = engine(Some(done));
    let mut outer = watch(task, watched(Duration::from_secs(30)));
    let Some(TaskOutcome::Done { metadata, .. }) = outer.outcomes.recv().await else {
        panic!("done");
    };
    let metadata = metadata.unwrap();
    assert_eq!(metadata["sample"]["partial"], false);
    assert_eq!(
        metadata["load_id"], "l",
        "the engine's own metadata is kept"
    );

    let (task, _) = engine(Some(TaskOutcome::Failed("source 500".into())));
    let mut outer = watch(task, watched(Duration::from_secs(30)));
    assert!(matches!(
        outer.outcomes.recv().await,
        Some(TaskOutcome::Failed(m)) if m == "source 500"
    ));
}

#[test]
fn the_run_detail_merges_the_ask_and_the_result() {
    let options = json!({ "pipeline_name": "p", "dataset_name": "raw_p", "window": null,
                          "resources": ["a"], "wall_clock_capped": true });
    let before = view(&options, None);
    assert_eq!(before["pipeline"], "p");
    assert_eq!(before["resources"], json!(["a"]));
    assert!(before.get("tables").is_none());
    let metadata = json!({ "sample": { "pipeline": "p", "tables": ["a"], "partial": false,
                                       "preview_pipeline": "preview:k:p" } });
    let after = view(&options, Some(&metadata));
    assert_eq!(after["tables"], json!(["a"]));
    assert_eq!(after["preview_pipeline"], "preview:k:p");
}

/// SHOULD-FIX 4: a table whose load writes airway's metadata into `main` — a
/// `replacing` resource, or one production holds as `replacing` or with a
/// watermark column — is refused at submit (`422 sample_unsupported`); a
/// sample of the pipeline's other resources is not.
#[test]
fn a_replacing_or_watermarked_table_is_unsupported() {
    let config = "    base_url: https://example.test\n    endpoints:\n      - name: orders\n        path: /orders\n        write_disposition: replacing\n        primary_key: [id]\n      - name: customers\n        path: /customers";
    let s = spec("rest_api", config, AIRHOUSE);
    let err = ask(&s, &["orders".into()], false).unwrap_err();
    assert_eq!(err.code(), "sample_unsupported", "{err}");
    assert!(err.to_string().contains("`main`"), "{err}");
    assert!(err.to_string().contains("orders"), "{err}");
    assert_eq!(
        RunRequestError::Sample(err).status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert!(
        ask(&s, &["customers".into()], false).is_ok(),
        "the other resource samples"
    );

    let stored: agentic_airway::schema_compat::Schema = serde_json::from_value(json!({
        "name": "p", "version": 1, "version_hash": "", "engine_version": 1,
        "tables": { "customers": { "name": "customers", "columns": {},
                                   "write_disposition": "merge", "business_column": "day" } }
    }))
    .unwrap();
    let asked = Asked {
        window: None,
        resources: &["customers".into()],
        has_sandbox: false,
    };
    let err = check(&s, &asked, Some(&stored), Utc::now()).unwrap_err();
    assert_eq!(
        err.code(),
        "sample_unsupported",
        "a watermark column production holds"
    );
}

/// SHOULD-FIX 1: the deadline runs from the preview run's start, so a task
/// claimed again later does not get a fresh one.
#[test]
fn the_deadline_runs_from_the_runs_start() {
    let cap = deadline(true, 900, 60);
    let now = Utc::now();
    let started = |mins: i64| Some((now - chrono::Duration::minutes(mins)).fixed_offset());
    assert_eq!(
        remaining(cap, started(1), now).after,
        Duration::from_secs(840)
    );
    assert_eq!(
        remaining(cap, started(20), now).after,
        Duration::ZERO,
        "already past it"
    );
    assert_eq!(
        remaining(cap, None, now).after,
        cap.after,
        "no start recorded: the whole cap"
    );
    assert_eq!(
        remaining(
            cap,
            Some((now + chrono::Duration::minutes(1)).fixed_offset()),
            now
        )
        .after,
        cap.after,
        "a clock skewed ahead is not negative time"
    );
}
