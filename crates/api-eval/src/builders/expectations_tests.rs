use oxy::config::test_config::{ExpectQuerySource, TestExpect};
use oxy::exec_types::{RunObserved, TargetOutput};

use super::*;

fn seen() -> RunObserved {
    RunObserved {
        tools: vec!["search".into(), "query".into()],
        query_sources: vec!["verified_sql".into()],
        answer_source: Some("verified_sql".into()),
        preagg: false,
        charts: 0,
        asked_user: None,
    }
}

fn asked(question: &str) -> RunObserved {
    RunObserved {
        asked_user: Some(question.into()),
        ..RunObserved::default()
    }
}

fn actual(observed: RunObserved) -> TargetOutput {
    TargetOutput {
        output: observed.asked_user.clone().unwrap_or_else(|| "42".into()),
        observed: Some(observed),
        ..Default::default()
    }
}

fn expected(expect: Option<TestExpect>, text: &str) -> TargetOutput {
    TargetOutput {
        output: text.into(),
        task_description: Some("q".into()),
        expect,
        ..Default::default()
    }
}

fn base(choice: &str, score: f32) -> Record {
    Record {
        cot: "judge says so".into(),
        choice: choice.into(),
        score,
        ..Default::default()
    }
}

#[test]
fn met_expectations_report_nothing() {
    let expect = TestExpect {
        asks_user: Some(false),
        chart: Some(false),
        preagg: Some(false),
        query_source: Some(ExpectQuerySource::VerifiedSql),
        tools: vec!["query".into()],
    };
    assert_eq!(unmet(&expect, &seen()), Vec::<String>::new());
    assert!(unmet(&TestExpect::default(), &seen()).is_empty());
}

#[test]
fn each_unmet_expectation_is_named() {
    let expect = TestExpect {
        asks_user: Some(true),
        chart: Some(true),
        preagg: Some(true),
        query_source: Some(ExpectQuerySource::Semantic),
        tools: vec!["chart".into()],
    };
    let unmet = unmet(&expect, &seen());
    let names: Vec<&str> = unmet.iter().map(|u| u.split(':').next().unwrap()).collect();
    assert_eq!(
        names,
        vec!["asks_user", "chart", "preagg", "query_source", "tools"]
    );
    assert!(
        unmet[3].contains("`semantic`") && unmet[3].contains("`verified_sql`"),
        "{unmet:?}"
    );
    assert!(
        unmet[4].contains("`chart` was never called (called: search, query)"),
        "{unmet:?}"
    );
}

#[test]
fn false_expectations_catch_the_behaviour() {
    let expect = TestExpect {
        chart: Some(false),
        preagg: Some(false),
        ..TestExpect::default()
    };
    let did = RunObserved {
        charts: 2,
        preagg: true,
        ..seen()
    };
    let unmet = unmet(&expect, &did);
    assert_eq!(unmet.len(), 2, "{unmet:?}");
    assert!(unmet[0].contains("2 drawn"), "{unmet:?}");
}

#[test]
fn a_query_source_with_no_answer_query_is_unmet() {
    let expect = TestExpect {
        query_source: Some(ExpectQuerySource::Llm),
        ..TestExpect::default()
    };
    let unmet = unmet(&expect, &RunObserved::default());
    assert!(unmet[0].contains("no successful query"), "{unmet:?}");
}

#[test]
fn an_unexpected_question_is_the_old_run_error() {
    let run = actual(asked("Which region?"));
    let msg = unexpected_question(&run, &expected(None, "x")).unwrap();
    assert_eq!(
        msg,
        "agentic pipeline asked a clarifying question: Which region?"
    );
    let not_asking = TestExpect {
        asks_user: Some(false),
        ..TestExpect::default()
    };
    assert!(unexpected_question(&run, &expected(Some(not_asking), "x")).is_some());
    let asking = TestExpect {
        asks_user: Some(true),
        ..TestExpect::default()
    };
    assert_eq!(unexpected_question(&run, &expected(Some(asking), "")), None);
    assert_eq!(
        unexpected_question(&actual(seen()), &expected(None, "x")),
        None
    );
}

#[test]
fn an_expected_question_is_not_answer_graded() {
    let chart = TestExpect {
        chart: Some(true),
        ..TestExpect::default()
    };
    let with_expect = expected(Some(chart), "");
    let plain = expected(None, "");
    assert!(!grades_answer(
        &actual(asked("Which region?")),
        &with_expect,
        true
    ));
    assert!(grades_answer(&actual(seen()), &with_expect, true));
    assert!(
        !grades_answer(&actual(seen()), &with_expect, false),
        "expectations and no reference: graded on the expectations alone"
    );
    assert!(
        grades_answer(&actual(seen()), &plain, false),
        "no expectations and no reference: graded (and failed) as before"
    );
    assert!(grades_answer(&TargetOutput::default(), &plain, true));
}

#[test]
fn an_expected_question_passes_on_expectations_alone() {
    let expect = TestExpect {
        asks_user: Some(true),
        ..TestExpect::default()
    };
    let run = actual(asked("Which region?"));
    let r = ungraded_judged(base("FAIL", 0.0), &run, &expected(Some(expect), ""));
    assert_eq!((r.choice.as_str(), r.score), ("PASS", 1.0));
    assert!(r.unmet_expectations.is_empty());
    let observed = r.observed.unwrap();
    assert_eq!(observed.asked_user.as_deref(), Some("Which region?"));
}

#[test]
fn an_answer_where_a_question_was_expected_fails() {
    let expect = TestExpect {
        asks_user: Some(true),
        ..TestExpect::default()
    };
    let r = ungraded_judged(
        base("FAIL", 0.0),
        &actual(seen()),
        &expected(Some(expect), ""),
    );
    assert_eq!((r.choice.as_str(), r.score), ("FAIL", 0.0));
    assert_eq!(r.unmet_expectations.len(), 1);
    assert!(
        r.cot.contains("Unmet expectations:\n- asks_user"),
        "{}",
        r.cot
    );
}

#[test]
fn an_unmet_expectation_fails_a_judge_pass() {
    let expect = TestExpect {
        chart: Some(true),
        ..TestExpect::default()
    };
    let mut r = base("PASS", 1.0);
    check_judged(&mut r, &actual(seen()), &expected(Some(expect), "42"));
    assert_eq!((r.choice.as_str(), r.score), ("FAIL", 0.0));
    assert!(
        r.cot.starts_with("judge says so\n\nUnmet expectations:"),
        "{}",
        r.cot
    );

    let mut r = base("PASS", 1.0);
    check_judged(&mut r, &actual(seen()), &expected(None, "42"));
    assert_eq!((r.choice.as_str(), r.score), ("PASS", 1.0));
    assert_eq!(r.cot, "judge says so");
    assert!(
        r.observed.is_some(),
        "observations are kept without expectations"
    );
}

#[test]
fn an_unmet_expectation_makes_an_execution_verdict_incorrect() {
    let expect = TestExpect {
        preagg: Some(true),
        ..TestExpect::default()
    };
    let mut r = Record {
        judge_verdict: Some(VERDICT_CORRECT.into()),
        judge_reason: Some("states 42".into()),
        ..base("PASS", 1.0)
    };
    check_executed(
        &mut r,
        &actual(seen()),
        &expected(Some(expect.clone()), "42"),
    );
    assert_eq!(r.choice, "FAIL");
    assert_eq!(r.score, 1.0, "the EX bit is left alone");
    assert_eq!(r.judge_verdict.as_deref(), Some(VERDICT_INCORRECT));
    let reason = r.judge_reason.unwrap();
    assert!(reason.starts_with("unmet expectations: preagg"), "{reason}");
    assert!(reason.ends_with("judge: states 42"), "{reason}");

    let mut skip = base("SKIP", 0.0);
    check_executed(&mut skip, &actual(seen()), &expected(Some(expect), "42"));
    assert_eq!(skip.choice, "SKIP");
    assert_eq!(skip.judge_verdict, None);
}

#[test]
fn an_expected_question_under_the_execution_grader_is_correct() {
    let expect = TestExpect {
        asks_user: Some(true),
        ..TestExpect::default()
    };
    let run = actual(asked("Which year?"));
    let r = ungraded_executed(base("FAIL", 0.0), &run, &expected(Some(expect), ""));
    assert_eq!(r.choice, "PASS");
    assert_eq!(r.judge_verdict.as_deref(), Some(VERDICT_CORRECT));
    assert_eq!(r.soft_f1, None);
}
