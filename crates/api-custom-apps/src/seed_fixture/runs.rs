//! The real runs a freshly built fixture queues, and what `oxy dev seed` prints.
//!
//! The runs are durable `app_function` tasks the server's worker executes on
//! production's build (r4), exactly as the console's Run now queues them. They
//! need the functions runtime, so a binary built without it queues none and
//! says so rather than reporting runs that will never execute.

use oxy::theme::StyledText;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::{APP_SLUG, Built, LIVE, Outcome, RELEASES};

/// The real runs queued once a fresh fixture is built, on production's build
/// (r4): the passing ones, and three that hit r4's defect.
const RUNS: &[(&str, &str)] = &[
    ("snapshot", "{}"),
    ("stores", "{}"),
    ("stores", "{}"),
    ("store", r#"{"store":7}"#),
    ("store", r#"{"store":7}"#),
    ("smoke", "{}"),
    ("smoke", "{}"),
    ("snapshot", "{}"),
    ("store", r#"{"store":9999}"#),
    ("store", r#"{"store":9999}"#),
    ("store", r#"{"store":9999}"#),
];

/// How many real runs a fresh fixture queues.
pub const QUEUED_RUNS: usize = RUNS.len();

#[cfg(feature = "custom-app-functions")]
pub(super) async fn queue_real_runs(
    conn: &DatabaseConnection,
    app_id: Uuid,
) -> Result<usize, String> {
    use crate::custom_apps_functions::{FunctionJobTrigger, trigger_function_job};
    for (name, input) in RUNS {
        let input: serde_json::Value = serde_json::from_str(input).map_err(|e| e.to_string())?;
        trigger_function_job(conn, app_id, name, Some(input), FunctionJobTrigger::Manual)
            .await
            .map_err(|e| format!("queue {name}: {e}"))?;
    }
    Ok(RUNS.len())
}

#[cfg(not(feature = "custom-app-functions"))]
pub(super) async fn queue_real_runs(
    _conn: &DatabaseConnection,
    _app_id: Uuid,
) -> Result<usize, String> {
    Err("the functions runtime is not built into this binary (custom-app-functions)".into())
}

pub(super) fn print_outcome(outcome: &Outcome) {
    match outcome {
        Outcome::Skipped(why) => println!("{} pulse-demo fixture skipped: {why}", "ℹ️".info()),
        Outcome::Foreign => println!(
            "{} local/{APP_SLUG} exists and is not the seeded fixture (its id differs); left alone",
            "⚠️".warning()
        ),
        Outcome::Present => println!(
            "{} pulse-demo fixture present ({RELEASES} builds and their history); nothing to do",
            "✅".success()
        ),
        Outcome::Built(built) => print_built(built),
    }
}

fn print_built(b: &Built) {
    let verb = if b.rebuilt { "rebuilt" } else { "built" };
    println!(
        "{} pulse-demo fixture {verb}: r1..r{LIVE} promoted to production (r{LIVE}, the bad \
         release, is live), r{RELEASES} on staging; {} seeded invocations, {} seeded vitals events",
        "✅".success(),
        b.invocations,
        b.events
    );
    match &b.queued {
        Ok(n) => println!("  {n} runs queued on r{LIVE} (the server's worker runs them)"),
        Err(why) => println!("  no runs queued: {why}"),
    }
    println!("  /admin/apps/local/{APP_SLUG}?section=builds");
    println!(
        "  no OLTP: `oxyc oltp provision --org local --writer app:pulse_demo` would give it a \
         store; this fixture does not use one"
    );
    if b.rebuilt {
        println!("  the server caches app lookups for 60 s: allow a minute before opening it");
    }
}
