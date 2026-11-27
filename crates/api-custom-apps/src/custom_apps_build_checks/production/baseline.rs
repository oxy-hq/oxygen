//! What production showed before a move: the baseline a production attempt's
//! `production_smoke` is judged against, handed to the runner in its claim.
//!
//! `production_smoke` does not ask whether production is perfect; it asks
//! what the move broke. "Before" is what the build's own staging checks saw
//! on production while they compared it with staging: the `changes` row of
//! the **newest staging attempt of that build whose comparison ran**
//! (`passed`) **against the build production served right before the move**.
//!
//! **Only that build's production is a baseline.** A comparison names the
//! production it saw (`production_build_id`), and the move being checked is
//! the newest move of production to this build (`super::moved::move_in`,
//! whose `previous` is the build it replaced). A comparison with any other
//! build describes a production the move did not replace, and is no baseline:
//!
//! - B is compared with P1, then P2 is promoted, then B: judged against P1,
//!   every error P2 already had would be B's;
//! - production is rolled back to A: A's comparison saw A's predecessor, so
//!   everything A always showed would be "new", and a second notice go out.
//!
//! Each of its `screens[]` entries is one screen at one viewport:
//!
//! | Baseline | Read from the `changes` row |
//! | --- | --- |
//! | `at` | when it got its verdict (`finished_at`) |
//! | `screens[].path`, `.viewport` | the same |
//! | `screens[].ready` | `production_ready` |
//! | `screens[].errors` | `production_errors`: the navigation error, if any, then the uncaught page errors, as exact strings, at most ten |
//!
//! The strings are kept exactly as stored: the runner compares them by
//! equality with what production throws now. `null` when no comparison of
//! the build with the build it replaced ever ran (it was the first build,
//! production served this same build, the comparison never finished, or it
//! saw another production). With none, or one that saw no screen
//! ([`Baseline::tells_apart`]), no error can be called new: the runner
//! reports the check `skipped`, and a `failed` one is still never rolled back
//! on (`super::rollback::decide`, from [`tells_apart_for`]).

use chrono::{DateTime, FixedOffset};
use entity::{app_build_checks, app_builds};
use sea_orm::{ConnectionTrait, DbErr};
use serde::Serialize;
use serde_json::Value;

use super::super::environment::CheckEnvironment;
use super::super::kind::{CheckKind, CheckStatus};
use super::super::store;
use super::moved::{self, BuildName};

/// The most errors one screen's baseline lists.
pub const MAX_ERRORS: usize = 10;

/// One screen at one viewport, as production showed it before the move.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BaselineScreen {
    pub path: String,
    pub viewport: String,
    pub ready: bool,
    pub errors: Vec<String>,
}

/// What the newest `changes` run of a build saw on production.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Baseline {
    /// When that run got its verdict.
    pub at: Option<DateTime<FixedOffset>>,
    pub screens: Vec<BaselineScreen>,
}

/// One `screens[]` entry of a `changes` detail; `None` for an entry that
/// names no screen or no viewport.
fn screen_of(entry: &Value) -> Option<BaselineScreen> {
    let text = |name: &str| entry.get(name).and_then(Value::as_str).map(str::to_string);
    let errors = entry
        .get("production_errors")
        .and_then(Value::as_array)
        .map(|errors| {
            errors
                .iter()
                .filter_map(Value::as_str)
                .take(MAX_ERRORS)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    Some(BaselineScreen {
        path: text("path")?,
        viewport: text("viewport")?,
        ready: entry.get("production_ready") == Some(&Value::Bool(true)),
        errors,
    })
}

impl Baseline {
    /// Whether it can tell a new error from an old one: it saw at least one
    /// screen. One that saw none tells nothing apart, like none at all, as
    /// the runner reads it (`oxyc`'s `scoreProductionSmoke`).
    pub fn tells_apart(&self) -> bool {
        !self.screens.is_empty()
    }
}

/// Whether a `changes` row compared its build with `before`: its detail
/// names the production it saw by publish id (what the runner writes), or by
/// row id.
fn compared_with(row: &app_build_checks::Model, before: &BuildName) -> bool {
    let detail = row.detail.as_ref();
    let seen = detail.and_then(|detail| detail.get("production_build_id"));
    seen.and_then(Value::as_str)
        .is_some_and(|seen| seen == before.build_id || seen == before.id.to_string())
}

/// The baseline in `rows` (every staging row of one build, any attempt) for
/// a move that replaced `before`: the newest comparison that ran against
/// that build.
pub fn from_rows(rows: &[app_build_checks::Model], before: &BuildName) -> Option<Baseline> {
    let ran = rows
        .iter()
        .filter(|row| row.kind == CheckKind::Changes.as_str())
        .filter(|row| row.status == CheckStatus::Passed.as_str())
        .filter(|row| compared_with(row, before))
        .max_by_key(|row| row.attempt)?;
    let screens = ran.detail.as_ref()?.get("screens")?.as_array()?;
    Some(Baseline {
        at: ran.finished_at,
        screens: screens.iter().filter_map(screen_of).collect(),
    })
}

/// The baseline of a production attempt of `build`: what its staging
/// comparison saw of the build production served right before the newest
/// move to `build`. `None` when that move replaced nothing, replaced a build
/// that is gone, or replaced a build no comparison of `build` saw.
pub async fn of_build<C: ConnectionTrait>(
    conn: &C,
    build: &app_builds::Model,
) -> Result<Option<Baseline>, DbErr> {
    let history = moved::history(conn, build.app_id).await?;
    let replaced = moved::move_in(&history, build.id).and_then(|moved| moved.previous);
    let Some(before) = moved::named(conn, replaced).await? else {
        return Ok(None);
    };
    let staging = CheckEnvironment::Staging.name();
    let rows = store::rows_of_build(conn, build.id, staging).await?;
    Ok(from_rows(&rows, &before))
}

/// Whether a production attempt of `build` has a baseline that tells new
/// errors apart, read by the platform itself: what decides whether a failed
/// `production_smoke` may roll production back, never the runner's word
/// alone.
pub async fn tells_apart_for<C: ConnectionTrait>(
    conn: &C,
    build: &app_builds::Model,
) -> Result<bool, DbErr> {
    Ok(of_build(conn, build)
        .await?
        .is_some_and(|baseline| baseline.tells_apart()))
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::json;
    use uuid::Uuid;

    use super::*;

    /// The build production served right before the move: the one the
    /// comparisons below name (`compared`).
    fn before() -> BuildName {
        BuildName {
            id: Uuid::from_u128(0xb4),
            build_id: "p-1".into(),
        }
    }

    fn from_rows(rows: &[app_build_checks::Model]) -> Option<Baseline> {
        super::from_rows(rows, &before())
    }

    fn row(kind: &str, attempt: i32, status: &str, detail: Value) -> app_build_checks::Model {
        let at = chrono::Utc
            .with_ymd_and_hms(2026, 11, 22, 10, attempt as u32, 0)
            .unwrap()
            .fixed_offset();
        app_build_checks::Model {
            id: Uuid::new_v4(),
            app_id: Uuid::from_u128(1),
            build_id: Uuid::from_u128(10),
            environment: "staging".into(),
            kind: kind.into(),
            attempt,
            status: status.into(),
            advisory: kind == "changes",
            summary: None,
            detail: Some(detail),
            notes: json!([]),
            run_id: None,
            runner: None,
            created_at: at,
            started_at: None,
            finished_at: Some(at),
        }
    }

    fn compared(errors: Value) -> Value {
        json!({ "findings": [], "production_build_id": "p-1", "compared": 2, "changed": 1,
            "screens": [
                { "path": "stores", "viewport": "desk", "changed": true, "ratio": 0.03,
                  "staging": "a.png", "production": "b.png", "diff": "c.png",
                  "production_ready": true, "production_errors": errors },
                { "path": "", "viewport": "phone", "changed": false, "ratio": 0,
                  "production_ready": false, "production_errors": [] },
                { "viewport": "desk" },
            ],
            "seen": null })
    }

    /// The contract's shape, from the newest attempt whose comparison ran.
    #[test]
    fn the_baseline_is_what_the_newest_comparison_saw_on_production() {
        let rows = vec![
            row("changes", 1, "passed", compared(json!(["old"]))),
            row(
                "smoke",
                2,
                "passed",
                json!({ "findings": [], "screens": [] }),
            ),
            row(
                "changes",
                2,
                "passed",
                compared(json!(["TypeError: x", 7, "ReferenceError: y"])),
            ),
            // A newer attempt whose comparison had nothing to compare with.
            row(
                "changes",
                3,
                "skipped",
                json!({ "findings": [], "screens": [] }),
            ),
        ];
        let baseline = from_rows(&rows).expect("a baseline");
        assert_eq!(
            serde_json::to_value(&baseline).unwrap(),
            json!({
                "at": "2026-11-22T10:02:00Z",
                "screens": [
                    { "path": "stores", "viewport": "desk", "ready": true,
                      "errors": ["TypeError: x", "ReferenceError: y"] },
                    { "path": "", "viewport": "phone", "ready": false, "errors": [] },
                ],
            })
        );
    }

    #[test]
    fn at_most_ten_errors_exactly_as_stored_and_none_when_nothing_was_compared() {
        let many: Vec<String> = (0..14).map(|i| format!("  E{i} \u{2026} ")).collect();
        let rows = vec![row("changes", 1, "passed", compared(json!(many)))];
        let errors = &from_rows(&rows).unwrap().screens[0].errors;
        assert_eq!(errors.len(), MAX_ERRORS);
        assert_eq!(errors[0], "  E0 \u{2026} ", "not trimmed, not cut");

        for none in [
            vec![],
            vec![row("smoke", 1, "passed", json!({ "findings": [] }))],
            vec![row("changes", 1, "skipped", compared(json!([])))],
            vec![row("changes", 1, "error", compared(json!([])))],
            vec![row("changes", 1, "running", compared(json!([])))],
            vec![row("changes", 1, "passed", json!({ "findings": [] }))],
        ] {
            assert_eq!(from_rows(&none), None);
        }
    }

    /// Only a comparison with the build the move replaced is a baseline: one
    /// that saw another production (it was replaced before this build's move,
    /// or this move is a rollback) describes a production the move did not
    /// replace.
    #[test]
    fn a_comparison_with_another_production_is_no_baseline() {
        let rows = vec![row("changes", 1, "passed", compared(json!(["old"])))];
        assert!(from_rows(&rows).is_some(), "it names p-1");
        let other = BuildName {
            id: Uuid::from_u128(0xb5),
            build_id: "p-2".into(),
        };
        assert_eq!(super::from_rows(&rows, &other), None);

        // Named by row id, it is the same build; naming none, it is no build.
        let mut by_row_id = compared(json!([]));
        by_row_id["production_build_id"] = json!(before().id);
        let rows = vec![row("changes", 1, "passed", by_row_id)];
        assert!(from_rows(&rows).is_some());
        let mut unnamed = compared(json!([]));
        unnamed["production_build_id"] = json!(null);
        assert_eq!(from_rows(&[row("changes", 1, "passed", unnamed)]), None);

        // The newest comparison with the build the move replaced, whatever a
        // later attempt compared with.
        let mut later = compared(json!(["seen on p-2"]));
        later["production_build_id"] = json!("p-2");
        let rows = vec![
            row("changes", 1, "passed", compared(json!(["seen on p-1"]))),
            row("changes", 2, "passed", later),
        ];
        let baseline = from_rows(&rows).expect("attempt 1 saw p-1");
        assert_eq!(baseline.screens[0].errors, ["seen on p-1"]);
        let baseline = super::from_rows(&rows, &other).expect("attempt 2 saw p-2");
        assert_eq!(baseline.screens[0].errors, ["seen on p-2"]);
    }

    /// A comparison that saw a screen tells new errors apart; one that saw
    /// none does not, though it is a baseline.
    #[test]
    fn only_a_baseline_that_saw_a_screen_tells_new_errors_apart() {
        let seen = vec![row("changes", 1, "passed", compared(json!([])))];
        assert!(from_rows(&seen).expect("a baseline").tells_apart());
        let blind = vec![row(
            "changes",
            1,
            "passed",
            json!({ "findings": [], "production_build_id": "p-1", "screens": [] }),
        )];
        assert!(!from_rows(&blind).expect("a baseline").tells_apart());
    }
}
