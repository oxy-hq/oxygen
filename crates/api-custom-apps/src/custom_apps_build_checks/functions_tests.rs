use super::*;

/// The build being checked, and another one staging moved on to.
const BUILD: Uuid = Uuid::from_u128(10);
const OTHER: Uuid = Uuid::from_u128(11);

/// A run whose invocation ran the build being checked.
fn reading(status: &str, answer: Option<&str>, error: Option<&str>) -> PlatformRunReading {
    PlatformRunReading {
        status: status.to_string(),
        answer: answer.map(str::to_string),
        error: error.map(str::to_string),
        invocation_id: Some(Uuid::from_u128(7)),
        ran_build: Some(BUILD),
        duration_ms: Some(1200),
    }
}

/// [`reading`], for a run whose invocation ran `ran` (or that wrote none).
fn ran(status: &str, ran: Option<Uuid>) -> Option<PlatformRunReading> {
    Some(PlatformRunReading {
        invocation_id: ran.map(|_| Uuid::from_u128(7)),
        ran_build: ran,
        ..reading(status, Some(r#"{"ok":true}"#), None)
    })
}

fn judged(status: &str, answer: Option<&str>, error: Option<&str>) -> FunctionResult {
    judge("smoke", "r1".into(), reading(status, answer, error))
}

fn queued(name: &str, run_id: &str) -> Started {
    Started {
        name: name.into(),
        run_id: Some(run_id.into()),
        error: None,
    }
}

fn not_queued(name: &str, why: &str) -> Started {
    Started {
        name: name.into(),
        run_id: None,
        error: Some(why.into()),
    }
}

const SOON: Duration = Duration::from_secs(4);

#[test]
fn a_run_passes_when_done_and_not_ok_false() {
    assert!(judged("done", Some(r#"{"ok":true}"#), None).passed);
    assert!(judged("done", Some("plain text"), None).passed);
    let not_ok = judged("done", Some(r#"{"ok":false,"why":"x"}"#), None);
    assert!(!not_ok.passed);
    assert_eq!(
        not_ok.error.as_deref(),
        Some("answer reported \"ok\": false")
    );
    let failed = judged("failed", None, Some("TypeError: boom"));
    assert!(!failed.passed);
    assert_eq!(failed.error.as_deref(), Some("TypeError: boom"));
    assert_eq!(
        judged("timed_out", None, None).error.as_deref(),
        Some("did not finish within 300s")
    );
}

#[test]
fn the_verdict_names_the_failed_functions() {
    let pass = judged("done", None, None);
    let mut fail = judged("failed", None, Some("boom"));
    fail.name = "reorder_math".into();
    let out = score(&[pass.clone(), fail]);
    assert_eq!(out.status, CheckStatus::Failed);
    assert_eq!(out.summary, "1 of 2 functions failed: reorder_math");
    assert_eq!(out.detail["findings"][0]["message"], "reorder_math: boom");
    assert_eq!(out.detail["functions"].as_array().unwrap().len(), 2);
    assert_eq!(score(&[pass]).summary, "1 of 1 functions passed");
    let only_refused = score(&[refused("x", "no build".into())]);
    assert_eq!(only_refused.status, CheckStatus::Error);
}

#[test]
fn the_started_runs_round_trip_through_a_running_rows_detail() {
    let started = vec![queued("smoke", "r1"), not_queued("stale", "not a check")];
    let detail = pending_detail(&started);
    assert_eq!(
        detail["findings"],
        json!([]),
        "a detail always has findings"
    );
    assert_eq!(pending_of(Some(&detail)), Some(started));
    // A row whose functions were never started, and a verdict's detail.
    assert_eq!(pending_of(None), None);
    assert_eq!(pending_of(Some(&json!({ "findings": [] }))), None);
    assert_eq!(pending_of(Some(&score(&[]).detail)), None);
}

#[test]
fn a_check_stays_running_until_every_run_is_terminal() {
    let started = vec![queued("a", "r1"), queued("b", "r2"), queued("c", "r3")];
    let going = [
        Some(reading("done", None, None)),
        Some(reading("running", None, None)),
        None,
    ];
    assert_eq!(
        settle(&started, &going, SOON, BUILD),
        Settled::Running { done: 1, total: 3 }
    );
    let finished = [
        Some(reading("done", None, None)),
        Some(reading("failed", None, Some("boom"))),
        Some(reading("done", Some(r#"{"ok":false}"#), None)),
    ];
    let Settled::Done(out) = settle(&started, &finished, SOON, BUILD) else {
        panic!("every run is terminal");
    };
    assert_eq!(out.status, CheckStatus::Failed);
    assert_eq!(out.summary, "2 of 3 functions failed: b, c");
    assert_eq!(out.detail["functions"][0]["passed"], true);
    assert_eq!(out.detail["functions"][1]["run_id"], "r2");
}

/// Each function's `duration_ms` is how long that function ran, as its own
/// invocation recorded it: never the time the check had been open when it
/// settled, which every function of the attempt would share.
#[test]
fn a_functions_duration_is_its_own_run_time() {
    let started = vec![
        queued("quick", "r1"),
        queued("slow", "r2"),
        queued("lost", "r3"),
    ];
    let took = |ms: u64| {
        let mut reading = ran("done", Some(BUILD))?;
        reading.duration_ms = Some(ms);
        Some(reading)
    };
    let readings = [
        took(35),
        took(8_200),
        // Still going at the deadline: it has no run time of its own yet.
        ran("running", Some(BUILD)).map(|mut reading| {
            reading.duration_ms = None;
            reading
        }),
    ];
    let waited = FUNCTIONS_DEADLINE + Duration::from_secs(7);
    let Settled::Done(out) = settle(&started, &readings, waited, BUILD) else {
        panic!("the deadline has passed");
    };
    let functions = &out.detail["functions"];
    assert_eq!(functions[0]["duration_ms"], 35);
    assert_eq!(functions[1]["duration_ms"], 8_200);
    assert_eq!(functions[2]["status"], "timed_out");
    assert_eq!(functions[2]["duration_ms"], json!(null));
    // One that could not be queued never ran.
    let refused = refused("stale", "not a check".into());
    assert_eq!(refused.duration_ms, None);
}

#[test]
fn past_the_deadline_a_run_still_going_is_timed_out() {
    let started = vec![queued("a", "r1"), queued("slow", "r2")];
    let readings = [
        Some(reading("done", None, None)),
        Some(reading("running", None, None)),
    ];
    let just_inside = FUNCTIONS_DEADLINE - Duration::from_millis(1);
    assert_eq!(
        settle(&started, &readings, just_inside, BUILD),
        Settled::Running { done: 1, total: 2 }
    );
    let Settled::Done(out) = settle(&started, &readings, FUNCTIONS_DEADLINE, BUILD) else {
        panic!("the deadline has passed");
    };
    assert_eq!(out.status, CheckStatus::Failed);
    assert_eq!(out.summary, "1 of 2 functions failed: slow");
    assert_eq!(out.detail["functions"][1]["status"], "timed_out");
    assert_eq!(
        out.detail["findings"][0]["message"],
        "slow: did not finish within 300s"
    );
    // A run nothing could read is timed out too, never passed.
    let Settled::Done(unread) = settle(&started[1..], &[None], FUNCTIONS_DEADLINE, BUILD) else {
        panic!("the deadline has passed");
    };
    assert_eq!(unread.status, CheckStatus::Failed);
}

#[test]
fn a_function_that_could_not_be_queued_is_refused_at_once() {
    let started = vec![not_queued("stale", "not marked \"check\": true")];
    let Settled::Done(out) = settle(&started, &[None], SOON, BUILD) else {
        panic!("nothing is running");
    };
    assert_eq!(
        out.status,
        CheckStatus::Error,
        "no verdict could be reached"
    );
    assert_eq!(out.detail["functions"][0]["status"], "refused");
    // Beside a run still going, it waits with it; beside a run that failed,
    // the check has a verdict.
    let mixed = vec![queued("a", "r1"), not_queued("stale", "x")];
    let going = [Some(reading("running", None, None)), None];
    assert_eq!(
        settle(&mixed, &going, SOON, BUILD),
        Settled::Running { done: 1, total: 2 }
    );
    let readings = [Some(reading("failed", None, Some("boom"))), None];
    let Settled::Done(out) = settle(&mixed, &readings, SOON, BUILD) else {
        panic!("every run is terminal");
    };
    assert_eq!(out.status, CheckStatus::Failed);
    assert_eq!(out.summary, "2 of 2 functions failed: a, stale");
}

/// Staging moved between the claim and the worker: a run whose invocation
/// ran another build, or a finished one that does not say which build it
/// ran, is no verdict on the build being checked, and never a pass.
#[test]
fn a_run_of_another_build_is_never_a_pass_for_this_one() {
    let started = vec![queued("a", "r1"), queued("b", "r2")];
    for stray in [ran("done", Some(OTHER)), ran("done", None)] {
        let said_which = stray.as_ref().unwrap().ran_build.is_some();
        let readings = [ran("done", Some(BUILD)), stray];
        let Settled::Done(out) = settle(&started, &readings, SOON, BUILD) else {
            panic!("every run is terminal");
        };
        assert_eq!(out.status, CheckStatus::Skipped, "judged by nobody");
        assert_eq!(out.detail["not_checked"], true, "{}", out.detail);
        assert_eq!(out.detail["findings"], json!([]));
        assert!(
            out.summary
                .starts_with("1 of 2 check functions ran on another build")
        );
        let functions = &out.detail["functions"];
        assert_eq!(functions[0]["passed"], true, "this build's own run");
        assert_eq!(functions[0]["ran_build"], json!(BUILD));
        assert_eq!(functions[1]["status"], NOT_THIS_BUILD);
        assert_eq!(functions[1]["passed"], false);
        let why = functions[1]["error"].as_str().unwrap();
        assert_eq!(why.starts_with("ran build 00000000-"), said_which, "{why}");
    }
    // A run still going on another build at the deadline is no timeout of
    // this build's.
    let readings = [ran("done", Some(BUILD)), ran("running", Some(OTHER))];
    let Settled::Done(out) = settle(&started, &readings, FUNCTIONS_DEADLINE, BUILD) else {
        panic!("the deadline has passed");
    };
    assert_eq!(out.detail["functions"][1]["status"], NOT_THIS_BUILD);
    assert_eq!(out.status, CheckStatus::Skipped);
}

/// A failure of this build's own run decides, whatever a stray run did; and a
/// run that ended without running anything is judged by how it ended.
#[test]
fn a_failure_of_this_builds_own_run_stands_beside_a_stray_one() {
    let started = vec![queued("a", "r1"), queued("b", "r2")];
    let mut failed = ran("failed", Some(BUILD)).unwrap();
    failed.error = Some("TypeError: boom".into());
    let readings = [Some(failed), ran("done", Some(OTHER))];
    let Settled::Done(out) = settle(&started, &readings, SOON, BUILD) else {
        panic!("every run is terminal");
    };
    assert_eq!(out.status, CheckStatus::Failed);
    assert!(out.detail.get("not_checked").is_none(), "{}", out.detail);
    assert_eq!(out.detail["findings"][0]["message"], "a: TypeError: boom");

    let never_ran = [ran("failed", None), ran("done", Some(BUILD))];
    let Settled::Done(out) = settle(&started, &never_ran, SOON, BUILD) else {
        panic!("every run is terminal");
    };
    assert_eq!(out.status, CheckStatus::Failed);
    assert_eq!(out.detail["functions"][0]["status"], "failed");
}
