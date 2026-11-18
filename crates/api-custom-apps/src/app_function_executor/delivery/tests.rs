use super::{AttemptOutcome, ListenerRun, attempt_of};

fn run<'a>(status: &'a str, http_status: u16, error: Option<&'a str>) -> ListenerRun<'a> {
    ListenerRun {
        status,
        http_status,
        error,
        memory_limit: false,
    }
}

#[test]
fn a_2xx_answer_is_ok() {
    for status in [200, 204, 299] {
        assert_eq!(
            attempt_of(&run("success", status, None)),
            (AttemptOutcome::Ok, None)
        );
    }
}

/// The SDK's `onEvent` answers 400 for a body that is not an envelope:
/// any answer outside 2xx is a failure, recorded by its status alone. The
/// listener's output never reaches Oxy's Postgres, so its body is not
/// quoted.
#[test]
fn any_other_answer_is_an_error_naming_its_status_only() {
    let (outcome, error) = attempt_of(&run("success", 400, None));
    assert_eq!(outcome, AttemptOutcome::Error);
    assert_eq!(error.as_deref(), Some("the listener answered HTTP 400"));
}

#[test]
fn a_timeout_and_a_heap_kill_are_their_own_outcomes() {
    let timeout = attempt_of(&run("timeout", 0, Some("function execution timed out")));
    assert_eq!(timeout.0, AttemptOutcome::Timeout);
    let oom = ListenerRun {
        memory_limit: true,
        ..run("error", 0, Some("function exceeded its memory limit"))
    };
    assert_eq!(attempt_of(&oom).0, AttemptOutcome::Crash);
    // Stopped from outside (a worker shutting down) or never started (shed):
    // the platform's, not the handler's.
    assert_eq!(
        attempt_of(&run("cancelled", 0, Some("function was cancelled"))).0,
        AttemptOutcome::Crash
    );
    assert_eq!(
        attempt_of(&run("shed", 0, Some("too many"))).0,
        AttemptOutcome::Crash
    );
}

#[test]
fn a_thrown_non_retryable_error_is_non_retryable_with_its_message() {
    for thrown in [
        "function threw: NonRetryableError: statement has no period\n    at default (oxy:function:3:9)\n    at async oxy:invoke:12:19",
        "function threw: NonRetryableError: statement has no period",
        // deno's fallback when the stack is a single line.
        "function threw: Uncaught NonRetryableError: statement has no period",
        "function threw: Uncaught (in promise) NonRetryableError: statement has no period",
    ] {
        assert_eq!(
            attempt_of(&run("error", 0, Some(thrown))),
            (
                AttemptOutcome::NonRetryable,
                Some("statement has no period".to_string())
            ),
            "{thrown}"
        );
    }
    // No message: the name is the reason.
    assert_eq!(
        attempt_of(&run(
            "error",
            0,
            Some("function threw: NonRetryableError\n    at x")
        )),
        (
            AttemptOutcome::NonRetryable,
            Some("NonRetryableError".to_string())
        )
    );
}

/// Exactly that name, as the error's own: a lookalike, or the name quoted
/// in another error's message, is an error the queue retries.
#[test]
fn only_the_exact_name_is_non_retryable() {
    for thrown in [
        "function threw: NonRetryableErrorish: x",
        "function threw: Error: NonRetryableError: x",
        "function threw: HostError: ctx.oltp: NonRetryableError: x",
        "function threw: nonretryableerror: x",
        "NonRetryableError: x",
        "module evaluation failed: NonRetryableError: x",
    ] {
        assert_eq!(
            attempt_of(&run("error", 0, Some(thrown))).0,
            AttemptOutcome::Error,
            "{thrown}"
        );
    }
}
