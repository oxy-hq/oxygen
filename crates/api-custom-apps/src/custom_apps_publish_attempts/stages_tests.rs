use std::time::Duration;

use serde_json::json;

use super::*;

fn names(timings: &[StageTiming]) -> Vec<&str> {
    timings.iter().map(|t| t.stage.as_str()).collect()
}

#[tokio::test]
async fn a_stage_that_succeeds_is_clocked_and_fails_nothing() {
    let mut timer = StageTimer::new();
    let out: Result<u8, ()> = timer.run(Stage::Admit, async { Ok(7) }).await;
    assert_eq!(out, Ok(7));
    assert_eq!(names(&timer.timings()), ["admit"]);
    assert_eq!(timer.failed(), None);
}

#[tokio::test]
async fn a_stage_that_errs_is_clocked_and_is_the_failed_one() {
    let mut timer = StageTimer::new();
    let _: Result<(), ()> = timer.run(Stage::Admit, async { Ok(()) }).await;
    let out: Result<(), &str> = timer.run(Stage::Inflate, async { Err("bad bundle") }).await;
    assert_eq!(out, Err("bad bundle"), "the error passes through untouched");
    assert_eq!(timer.failed(), Some(Stage::Inflate));
    assert_eq!(
        names(&timer.timings()),
        ["admit", "inflate"],
        "the failed stage's time is kept, and nothing after it ran"
    );
}

#[tokio::test]
async fn the_first_failure_is_the_one_recorded() {
    let mut timer = StageTimer::new();
    let _: Result<(), ()> = timer.run(Stage::Store, async { Err(()) }).await;
    let _: Result<(), ()> = timer.run(Stage::Record, async { Err(()) }).await;
    assert_eq!(timer.failed(), Some(Stage::Store));
}

#[tokio::test]
async fn an_infallible_stage_is_clocked_and_never_fails() {
    let mut timer = StageTimer::new();
    let warnings = timer
        .run_infallible(Stage::After, async { vec!["w"] })
        .await;
    assert_eq!(warnings, ["w"]);
    assert_eq!(names(&timer.timings()), ["after"]);
    assert_eq!(timer.failed(), None);
}

#[test]
fn a_stage_run_in_pieces_adds_up() {
    let mut timer = StageTimer::new();
    timer.clock(Stage::Store, Duration::from_millis(40));
    timer.clock(Stage::Store, Duration::from_millis(2));
    assert_eq!(
        timer.timings(),
        [StageTiming {
            stage: "store".into(),
            ms: 42
        }]
    );
}

#[test]
fn timings_come_out_in_pipeline_order_whatever_order_they_were_clocked() {
    let mut timer = StageTimer::new();
    for stage in Stage::ALL.into_iter().rev() {
        timer.clock(stage, Duration::from_millis(1));
    }
    assert_eq!(
        names(&timer.timings()),
        [
            "admit", "inflate", "store", "record", "gate", "promote", "point", "after"
        ]
    );
}

#[test]
fn every_stage_round_trips_through_its_name() {
    for stage in Stage::ALL {
        assert_eq!(Stage::from_name(stage.name()), Some(stage));
    }
    assert_eq!(Stage::from_name("deploy"), None);
}

#[test]
fn stored_timings_read_back_in_pipeline_order() {
    let mut timer = StageTimer::new();
    timer.clock(Stage::Point, Duration::from_millis(30));
    timer.clock(Stage::Admit, Duration::from_millis(12));
    let stored = timer.to_json();
    assert_eq!(stored, json!({ "admit": 12, "point": 30 }));
    assert_eq!(timings_from_json(&stored), timer.timings());
}

/// A row written by a newer release names a stage this binary does not know:
/// it is kept, after the known ones, and a value that is not a duration is not.
#[test]
fn an_unknown_stage_follows_the_known_ones_and_junk_is_skipped() {
    let stored = json!({ "zeta": 1, "point": 3, "admit": 2, "beta": 4, "store": "slow" });
    assert_eq!(
        timings_from_json(&stored),
        [
            StageTiming {
                stage: "admit".into(),
                ms: 2
            },
            StageTiming {
                stage: "point".into(),
                ms: 3
            },
            StageTiming {
                stage: "beta".into(),
                ms: 4
            },
            StageTiming {
                stage: "zeta".into(),
                ms: 1
            },
        ]
    );
    assert!(timings_from_json(&json!(null)).is_empty());
    assert!(timings_from_json(&json!([1, 2])).is_empty());
}

/// The pipeline's header names its stages in order; `Stage::ALL` must be that
/// list, so a stage added, renamed or reordered in one is in the other.
#[test]
fn the_stages_are_the_ones_the_pipeline_header_names() {
    let source = include_str!("../custom_apps_publish/pipeline.rs");
    let header = source
        .lines()
        .take_while(|line| line.starts_with("//!"))
        .map(|line| line.trim_start_matches("//!").trim())
        .collect::<Vec<_>>()
        .join(" ");
    let expected = Stage::ALL
        .iter()
        .map(|stage| stage.name())
        .collect::<Vec<_>>()
        .join(" → ");
    assert!(
        header.contains(&expected),
        "pipeline.rs's header should name `{expected}`; it reads: {header}"
    );
}
