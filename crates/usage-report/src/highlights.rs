//! What in a week's numbers is worth a person's attention.
//!
//! Pure, and computed when a report is read rather than when it is stored, so a
//! reader bounded to some orgs is told about those orgs only, and a rule changed
//! here applies to the reports already written.
//!
//! The thresholds are small on purpose. These apps are internal tools with a
//! handful of users each, so a rule that waits for hundreds of people never
//! fires; one that fires on a single person coming or going never stops.

use serde::Serialize;
use uuid::Uuid;

use super::model::{AppUsage, OrgUsage, Snapshot, plural};
use super::period::Period;

/// People who must have used an app the week before for its silence to be news.
const QUIET_MIN_PEOPLE_BEFORE: u64 = 2;
/// A rise or fall is called out only when at least this many people moved…
const TREND_MIN_PEOPLE_MOVED: u64 = 3;
/// …and the app lost half its people, or gained half as many again.
const DROP_TO_AT_MOST: (u64, u64) = (1, 2);
const GROW_TO_AT_LEAST: (u64, u64) = (3, 2);
/// Function calls failing: one in ten once there are enough calls to trust the
/// ratio, or at least five failures that are half of everything.
const FAILING_MIN_CALLS: u64 = 20;
const FAILING_RATIO: (u64, u64) = (1, 10);
const FAILING_FEW_MIN_FAILURES: u64 = 5;
/// Page errors: at least three sessions, and a quarter of all of them.
const ERRORS_MIN_SESSIONS: u64 = 3;
const ERRORS_RATIO: (u64, u64) = (1, 4);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    // Declared in the order a report lists them.
    WentQuiet,
    FailingFunctions,
    ClientErrors,
    Dropping,
    Growing,
    FirstWeek,
    Unused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    /// Worth a look this week.
    Attention,
    Good,
    /// Published and unopened: true, and rarely urgent.
    Idle,
}

impl Kind {
    pub fn tone(self) -> Tone {
        match self {
            Kind::WentQuiet | Kind::FailingFunctions | Kind::ClientErrors | Kind::Dropping => {
                Tone::Attention
            }
            Kind::Growing | Kind::FirstWeek => Tone::Good,
            Kind::Unused => Tone::Idle,
        }
    }

    /// What the kind is called where a highlight is shown.
    pub fn label(self) -> &'static str {
        match self {
            Kind::WentQuiet => "Went quiet",
            Kind::FailingFunctions => "Functions failing",
            Kind::ClientErrors => "Page errors",
            Kind::Dropping => "Fewer people",
            Kind::Growing => "More people",
            Kind::FirstWeek => "First week",
            Kind::Unused => "Not opened",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Highlight {
    pub kind: Kind,
    pub tone: Tone,
    pub org_id: Uuid,
    pub org_name: String,
    pub org_slug: String,
    pub app_id: Uuid,
    pub app_name: String,
    pub app_slug: String,
    /// The kind, in words.
    pub label: &'static str,
    /// The numbers behind it, as a few words (`9 → 3 people`). Shown beside
    /// the label, as written, by the email and the console both.
    pub detail: String,
    /// How much the thing moved, for ordering within a kind. Not serialized.
    #[serde(skip)]
    weight: u64,
}

/// Every highlight in the snapshot: attention first, then good news, then the
/// idle apps; the biggest of each kind first.
pub fn highlights(snapshot: &Snapshot) -> Vec<Highlight> {
    let mut found: Vec<Highlight> = Vec::new();
    for org in &snapshot.orgs {
        for app in &org.apps {
            let noted = [
                trend(app, &snapshot.period),
                failing_functions(app),
                client_errors(app),
            ];
            for (kind, weight, detail) in noted.into_iter().flatten() {
                found.push(highlight(org, app, kind, weight, detail));
            }
        }
    }
    found.sort_by(|a, b| {
        (a.kind, std::cmp::Reverse(a.weight), &a.app_name).cmp(&(
            b.kind,
            std::cmp::Reverse(b.weight),
            &b.app_name,
        ))
    });
    found
}

type Noted = Option<(Kind, u64, String)>;

fn highlight(org: &OrgUsage, app: &AppUsage, kind: Kind, weight: u64, detail: String) -> Highlight {
    Highlight {
        kind,
        tone: kind.tone(),
        label: kind.label(),
        org_id: org.org_id,
        org_name: org.name.clone(),
        org_slug: org.slug.clone(),
        app_id: app.app_id,
        app_name: app.name.clone(),
        app_slug: app.slug.clone(),
        detail,
        weight,
    }
}

/// `part / whole >= ratio`, without the division.
fn at_least(part: u64, whole: u64, (num, den): (u64, u64)) -> bool {
    part * den >= whole * num
}

/// `part / whole <= ratio`.
fn at_most(part: u64, whole: u64, (num, den): (u64, u64)) -> bool {
    part * den <= whole * num
}

/// How the number of people changed. At most one of these is true of an app.
fn trend(app: &AppUsage, period: &Period) -> Noted {
    let (now, before) = (app.current.people, app.previous.people);
    if now == 0 && before >= QUIET_MIN_PEOPLE_BEFORE {
        let detail = format!("{} → nobody", plural(before, "person", "people"));
        return Some((Kind::WentQuiet, before, detail));
    }
    if now == 0 && before == 0 {
        return unused(app, period);
    }
    if app.first_week && now > 0 {
        return Some((Kind::FirstWeek, now, plural(now, "person", "people")));
    }
    let moved = now.abs_diff(before);
    if moved < TREND_MIN_PEOPLE_MOVED {
        return None;
    }
    let change = format!("{before} → {}", plural(now, "person", "people"));
    if now < before && at_most(now, before, DROP_TO_AT_MOST) {
        return Some((Kind::Dropping, moved, change));
    }
    if now > before && before > 0 && at_least(now, before, GROW_TO_AT_LEAST) {
        return Some((Kind::Growing, moved, change));
    }
    None
}

/// Live for two full weeks with nobody opening it. An app published during
/// those weeks is left alone: it has not had the time.
fn unused(app: &AppUsage, period: &Period) -> Noted {
    let published_before = app
        .published_at
        .is_some_and(|at| at < period.previous().start);
    published_before.then(|| (Kind::Unused, 0, "No opens in two weeks".to_string()))
}

fn failing_functions(app: &AppUsage) -> Noted {
    let (calls, failed) = (app.current.function_calls, app.current.function_failures);
    let many = calls >= FAILING_MIN_CALLS && at_least(failed, calls, FAILING_RATIO);
    let few = failed >= FAILING_FEW_MIN_FAILURES && at_least(failed, calls, (1, 2));
    (failed > 0 && (many || few)).then(|| {
        let detail = format!("{failed} of {} failed", plural(calls, "call", "calls"));
        (Kind::FailingFunctions, failed, detail)
    })
}

fn client_errors(app: &AppUsage) -> Noted {
    let errored = app.current.error_sessions;
    // Errors and sessions come from different tables; a session can report an
    // error without a recorded view, so never divide by fewer than the errors.
    let sessions = app.current.sessions.max(errored);
    (errored >= ERRORS_MIN_SESSIONS && at_least(errored, sessions, ERRORS_RATIO)).then(|| {
        let detail = format!(
            "{errored} of {} hit an error",
            plural(sessions, "visit", "visits")
        );
        (Kind::ClientErrors, errored, detail)
    })
}

#[cfg(test)]
#[path = "highlights_tests.rs"]
mod tests;
