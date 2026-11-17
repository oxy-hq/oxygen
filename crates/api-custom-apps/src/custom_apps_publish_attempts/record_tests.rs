use chrono::Utc;
use entity::app_publish_attempts::{OUTCOME_FAILED, OUTCOME_SUCCESS};
use sea_orm::ActiveValue;
use uuid::Uuid;

use super::*;

const IDENTITY: &str = "github-oidc:acme/app/.github/workflows/p.yml@refs/heads/main";

fn facts() -> AttemptFacts {
    AttemptFacts {
        app_slug: "store-pulse".into(),
        build_id: "ci-42".into(),
        environment: "staging".into(),
        published_by: None,
        published_via: Some(IDENTITY.into()),
        published_token_id: None,
    }
}

fn set<V: Clone + Into<sea_orm::Value>>(value: &ActiveValue<V>) -> V {
    value
        .try_as_ref()
        .cloned()
        .expect("the row sets every column")
}

fn stage_names(row: &app_publish_attempts::ActiveModel) -> Vec<String> {
    let stored = set(&row.stage_ms);
    stored
        .as_object()
        .expect("{stage: ms}")
        .keys()
        .cloned()
        .collect()
}

/// Run the pipeline's stages through the real timer, the one named by
/// `fails_at` refusing, as the pipeline would: nothing after it runs.
async fn ran(fails_at: Option<Stage>) -> Attempt {
    let mut attempt = Attempt::begin(facts());
    attempt.authorized_org = Some(Uuid::new_v4());
    attempt.app_id = Some(Uuid::new_v4());
    for stage in Stage::ALL {
        let refused = Some(stage) == fails_at;
        let out: Result<(), ()> = attempt
            .clock
            .run(stage, async move { if refused { Err(()) } else { Ok(()) } })
            .await;
        if out.is_err() {
            return attempt;
        }
        if stage == Stage::Record {
            // The build row exists from here on; a later failure undoes it.
            attempt.app_build_id = Some(Uuid::new_v4());
        }
    }
    attempt
}

#[tokio::test]
async fn a_publish_that_lands_records_its_build_and_every_stage() {
    let attempt = ran(None).await;
    let app_id = Uuid::new_v4();
    let row = attempt.row(app_id, None, Utc::now());

    assert_eq!(set(&row.outcome), OUTCOME_SUCCESS);
    assert_eq!(set(&row.app_id), app_id);
    assert_eq!(set(&row.app_build_id), attempt.app_build_id);
    assert!(set(&row.app_build_id).is_some());
    assert_eq!(set(&row.failed_stage), None);
    assert_eq!(set(&row.error), None);
    assert_eq!(set(&row.build_id), "ci-42");
    assert_eq!(set(&row.environment), "staging");
    assert_eq!(set(&row.published_via).as_deref(), Some(IDENTITY));
    let mut every: Vec<String> = Stage::ALL.iter().map(|s| s.name().to_string()).collect();
    every.sort();
    let mut stored = stage_names(&row);
    stored.sort();
    assert_eq!(
        stored, every,
        "every stage is timed on a publish that lands"
    );
    assert!(set(&row.finished_at) >= set(&row.started_at));
}

/// Every stage that can refuse a publish: the attempt names it, keeps the
/// refusal, times the stages up to and including it, and names no build row —
/// even when the failure came after the row was recorded (the undo deleted it).
#[tokio::test]
async fn a_failure_at_each_stage_is_recorded_at_that_stage() {
    let fallible = Stage::ALL.into_iter().filter(|s| *s != Stage::After);
    for stage in fallible {
        let attempt = ran(Some(stage)).await;
        let row = attempt.row(Uuid::new_v4(), Some("refused"), Utc::now());

        assert_eq!(set(&row.outcome), OUTCOME_FAILED, "{stage:?}");
        assert_eq!(
            set(&row.failed_stage).as_deref(),
            Some(stage.name()),
            "{stage:?}"
        );
        assert_eq!(set(&row.error).as_deref(), Some("refused"), "{stage:?}");
        assert_eq!(set(&row.app_build_id), None, "{stage:?}");
        let through: Vec<&str> = Stage::ALL
            .iter()
            .take_while(|s| **s != stage)
            .chain(std::iter::once(&stage))
            .map(|s| s.name())
            .collect();
        let mut expected: Vec<String> = through.iter().map(|s| s.to_string()).collect();
        expected.sort();
        let mut stored = stage_names(&row);
        stored.sort();
        assert_eq!(stored, expected, "{stage:?}: the stages that ran, no more");
    }
}

#[test]
fn a_long_refusal_is_cut_and_says_so() {
    let long = "é".repeat(MAX_ERROR_CHARS + 10);
    let cut = capped(&long, MAX_ERROR_CHARS);
    assert!(cut.ends_with("… (truncated)"));
    assert_eq!(
        cut.trim_end_matches("… (truncated)").chars().count(),
        MAX_ERROR_CHARS,
        "cut on a character boundary, not a byte one"
    );
    let short = "invalid bundle: index.html has no <head>";
    assert_eq!(capped(short, MAX_ERROR_CHARS), short);
}

/// Every string a caller sends that the row stores is bounded: a build id far
/// past the valid length (refused, but still recorded), a long environment
/// name, a long workflow identity and the refusal that quotes them.
#[test]
fn every_caller_supplied_string_is_capped() {
    let huge = "x".repeat(100_000);
    let mut attempt = Attempt::begin(AttemptFacts {
        app_slug: "store-pulse".into(),
        build_id: huge.clone(),
        environment: huge.clone(),
        published_by: None,
        published_via: Some(huge.clone()),
        published_token_id: None,
    });
    attempt.clock.fail(Stage::Admit);
    let row = attempt.row(Uuid::new_v4(), Some(&huge), Utc::now());
    let chars = |s: String| s.trim_end_matches("… (truncated)").chars().count();
    assert_eq!(chars(set(&row.build_id)), MAX_BUILD_ID_CHARS);
    assert_eq!(chars(set(&row.environment)), MAX_LABEL_CHARS);
    assert_eq!(
        chars(set(&row.published_via).expect("a workflow")),
        MAX_LABEL_CHARS
    );
    assert_eq!(chars(set(&row.error).expect("a refusal")), MAX_ERROR_CHARS);
}
