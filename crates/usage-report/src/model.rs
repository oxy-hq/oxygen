//! What a report holds, and the two things read off it: the slice a reader may
//! see, and the totals that lead it.
//!
//! Everything here is counts and the names they belong to. Nothing a person did
//! inside an app is in a report.

use chrono::{DateTime, Utc};
use oxy_authz::Scope;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::period::Period;

/// One app's numbers for one week.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    /// Times the app's page was served to someone signed in.
    pub views: u64,
    /// Distinct people who opened it.
    pub people: u64,
    pub sessions: u64,
    /// Sessions in which the page reported an error.
    pub error_sessions: u64,
    pub function_calls: u64,
    /// Calls that ended in an error or a timeout.
    pub function_failures: u64,
    /// Builds promoted to production.
    #[serde(default)]
    pub releases: u64,
}

impl Counts {
    pub fn any_activity(&self) -> bool {
        self.views > 0 || self.function_calls > 0 || self.releases > 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppUsage {
    pub app_id: Uuid,
    pub name: String,
    pub slug: String,
    /// When the app was last published; `None` for one that is not live.
    pub published_at: Option<DateTime<Utc>>,
    /// Nobody had opened it before this week, as far back as views are kept.
    #[serde(default)]
    pub first_week: bool,
    /// Size of the app's stored files at the end of the week. `None` is "never
    /// measured", which is not zero.
    #[serde(default)]
    pub storage_bytes: Option<u64>,
    /// The same at the start of the week.
    #[serde(default)]
    pub storage_bytes_before: Option<u64>,
    pub current: Counts,
    pub previous: Counts,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgUsage {
    pub org_id: Uuid,
    pub name: String,
    pub slug: String,
    /// Distinct people across the org's apps — not the sum of each app's, since
    /// one person may open several.
    pub people: u64,
    pub prev_people: u64,
    pub apps: Vec<AppUsage>,
}

impl OrgUsage {
    pub fn views(&self) -> u64 {
        self.apps.iter().map(|a| a.current.views).sum()
    }

    pub fn prev_views(&self) -> u64 {
        self.apps.iter().map(|a| a.previous.views).sum()
    }

    pub fn function_calls(&self) -> u64 {
        self.apps.iter().map(|a| a.current.function_calls).sum()
    }

    pub fn function_failures(&self) -> u64 {
        self.apps.iter().map(|a| a.current.function_failures).sum()
    }

    pub fn releases(&self) -> u64 {
        self.apps.iter().map(|a| a.current.releases).sum()
    }

    /// What the org's apps store between them; `None` when none was measured.
    pub fn storage_bytes(&self) -> Option<u64> {
        measured(self.apps.iter().map(|a| a.storage_bytes))
    }
}

/// The sum of the sizes that were measured, or `None` if none was — an
/// unmeasured app is left out of a total rather than counted as empty.
fn measured(sizes: impl Iterator<Item = Option<u64>>) -> Option<u64> {
    sizes
        .flatten()
        .fold(None, |sum, b| Some(sum.unwrap_or(0) + b))
}

/// One week of usage for every org with an app worth reporting. This is what is
/// stored; highlights and totals are computed from it when it is read, so a
/// reader bounded to some orgs gets numbers for those orgs only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub period: Period,
    pub orgs: Vec<OrgUsage>,
}

impl Snapshot {
    /// The part of the report `scope` reaches. An unbounded scope sees all of it.
    pub fn scoped(&self, scope: &Scope) -> Snapshot {
        Snapshot {
            period: self.period,
            orgs: self
                .orgs
                .iter()
                .filter(|org| scope.covers(org.org_id))
                .cloned()
                .collect(),
        }
    }

    /// Nobody opened anything in either week: there is nothing to tell anyone.
    pub fn is_silent(&self) -> bool {
        self.orgs
            .iter()
            .all(|o| o.people == 0 && o.prev_people == 0)
    }

    pub fn summary(&self) -> Summary {
        let apps = || self.orgs.iter().flat_map(|o| o.apps.iter());
        let people = self.orgs.iter().map(|o| o.people).sum();
        let prev_people = self.orgs.iter().map(|o| o.prev_people).sum();
        let active_apps = apps().filter(|a| a.current.people > 0).count() as u64;
        let active_orgs = self.orgs.iter().filter(|o| o.people > 0).count() as u64;
        Summary {
            headline: headline(people, active_apps, active_orgs),
            comparison: comparison(people, prev_people),
            people,
            prev_people,
            views: self.orgs.iter().map(OrgUsage::views).sum(),
            prev_views: self.orgs.iter().map(OrgUsage::prev_views).sum(),
            active_apps,
            apps: apps().count() as u64,
            active_orgs,
            orgs: self.orgs.len() as u64,
            function_calls: self.orgs.iter().map(OrgUsage::function_calls).sum(),
            function_failures: self.orgs.iter().map(OrgUsage::function_failures).sum(),
            releases: self.orgs.iter().map(OrgUsage::releases).sum(),
            storage_bytes: measured(apps().map(|a| a.storage_bytes)),
            storage_bytes_before: measured(apps().map(|a| a.storage_bytes_before)),
        }
    }
}

/// The totals that lead a report. `people` adds each org's distinct people, so
/// someone who belongs to two orgs counts in both.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Summary {
    pub headline: String,
    pub comparison: String,
    pub people: u64,
    pub prev_people: u64,
    pub views: u64,
    pub prev_views: u64,
    /// Apps somebody opened, of `apps` in the report.
    pub active_apps: u64,
    pub apps: u64,
    /// Orgs where somebody opened an app, of `orgs` in the report.
    pub active_orgs: u64,
    pub orgs: u64,
    pub function_calls: u64,
    pub function_failures: u64,
    /// Builds promoted to production this week.
    pub releases: u64,
    /// Everything the apps store, where measured.
    pub storage_bytes: Option<u64>,
    pub storage_bytes_before: Option<u64>,
}

/// `3 people`, `1 person`.
pub(super) fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn headline(people: u64, active_apps: u64, active_orgs: u64) -> String {
    if people == 0 {
        return "Nobody opened a custom app this week.".to_string();
    }
    format!(
        "{} opened {} across {}.",
        plural(people, "person", "people"),
        plural(active_apps, "custom app", "custom apps"),
        plural(active_orgs, "organization", "organizations"),
    )
}

fn comparison(people: u64, prev_people: u64) -> String {
    match (people, prev_people) {
        (0, 0) => "Nobody did the week before either.".to_string(),
        (_, 0) => "Nobody did the week before.".to_string(),
        (now, before) if now == before => "The same number as the week before.".to_string(),
        (now, before) if now > before => changed(now - before, "more"),
        (now, before) => changed(before - now, "fewer"),
    }
}

/// `That is 3 more people than the week before.`
fn changed(by: u64, direction: &str) -> String {
    let noun = if by == 1 { "person" } else { "people" };
    format!("That is {by} {direction} {noun} than the week before.")
}

#[cfg(test)]
pub(super) mod fixtures {
    use super::*;
    use chrono::TimeZone;

    pub fn period() -> Period {
        Period::last_completed(Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap())
    }

    pub fn counts(people: u64) -> Counts {
        Counts {
            views: people * 4,
            people,
            sessions: people * 2,
            ..Counts::default()
        }
    }

    /// An app published long ago, with `now` people this week and `before` the
    /// week before.
    pub fn app(n: u128, name: &str, now: u64, before: u64) -> AppUsage {
        AppUsage {
            app_id: Uuid::from_u128(n),
            name: name.to_string(),
            slug: name.to_lowercase().replace(' ', "-"),
            published_at: Some(Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap()),
            first_week: false,
            storage_bytes: None,
            storage_bytes_before: None,
            current: counts(now),
            previous: counts(before),
        }
    }

    pub fn org(n: u128, name: &str, apps: Vec<AppUsage>) -> OrgUsage {
        OrgUsage {
            org_id: Uuid::from_u128(n),
            name: name.to_string(),
            slug: name.to_lowercase().replace(' ', "-"),
            // Nobody in these fixtures opens two apps.
            people: apps.iter().map(|a| a.current.people).sum(),
            prev_people: apps.iter().map(|a| a.previous.people).sum(),
            apps,
        }
    }

    pub fn snapshot(orgs: Vec<OrgUsage>) -> Snapshot {
        Snapshot {
            period: period(),
            orgs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    fn two_orgs() -> Snapshot {
        snapshot(vec![
            org(
                1,
                "Poke House",
                vec![app(10, "Store Ops", 6, 4), app(11, "Checklists", 0, 0)],
            ),
            org(2, "Rivermark", vec![app(20, "Crew Board", 3, 9)]),
        ])
    }

    #[test]
    fn the_summary_counts_people_apps_and_orgs_that_were_active() {
        let s = two_orgs().summary();
        assert_eq!(
            s.headline,
            "9 people opened 2 custom apps across 2 organizations."
        );
        assert_eq!(s.comparison, "That is 4 fewer people than the week before.");
        assert_eq!((s.people, s.prev_people), (9, 13));
        assert_eq!((s.active_apps, s.apps), (2, 3));
        assert_eq!((s.active_orgs, s.orgs), (2, 2));
        assert_eq!((s.views, s.prev_views), (36, 52));
    }

    #[test]
    fn the_headline_reads_as_a_sentence_at_one_and_at_none() {
        let one = snapshot(vec![org(1, "Poke House", vec![app(10, "Store Ops", 1, 0)])]);
        assert_eq!(
            one.summary().headline,
            "1 person opened 1 custom app across 1 organization."
        );
        assert_eq!(one.summary().comparison, "Nobody did the week before.");

        let none = snapshot(vec![org(1, "Poke House", vec![app(10, "Store Ops", 0, 0)])]);
        assert_eq!(
            none.summary().headline,
            "Nobody opened a custom app this week."
        );
        assert_eq!(
            none.summary().comparison,
            "Nobody did the week before either."
        );
        assert!(none.is_silent());
    }

    #[test]
    fn the_comparison_names_the_direction() {
        assert_eq!(
            comparison(8, 7),
            "That is 1 more person than the week before."
        );
        assert_eq!(comparison(5, 5), "The same number as the week before.");
    }

    #[test]
    fn a_bounded_scope_sees_only_its_orgs_and_their_totals() {
        let all = two_orgs();
        let rivermark = all.scoped(&Scope::Orgs(vec![Uuid::from_u128(2)]));
        assert_eq!(rivermark.orgs.len(), 1);
        assert_eq!(rivermark.orgs[0].name, "Rivermark");
        assert_eq!(rivermark.summary().people, 3);
        assert_eq!(rivermark.summary().orgs, 1);

        assert_eq!(all.scoped(&Scope::All), all);
        // A grant that names no org reaches nothing.
        assert!(all.scoped(&Scope::Orgs(vec![])).orgs.is_empty());
    }

    #[test]
    fn a_week_with_no_one_only_reads_as_silent_when_the_week_before_was_too() {
        let went_quiet = snapshot(vec![org(1, "Poke House", vec![app(10, "Store Ops", 0, 5)])]);
        assert!(!went_quiet.is_silent());
    }

    #[test]
    fn a_snapshot_stored_before_a_field_existed_still_reads() {
        let mut stored = serde_json::to_value(two_orgs()).unwrap();
        let app = stored["orgs"][0]["apps"][0].as_object_mut().unwrap();
        app.remove("first_week");
        app.remove("storage_bytes");
        app.remove("storage_bytes_before");
        app["current"].as_object_mut().unwrap().remove("releases");
        let read: Snapshot = serde_json::from_value(stored).unwrap();
        let app = &read.orgs[0].apps[0];
        assert!(!app.first_week);
        assert_eq!((app.storage_bytes, app.current.releases), (None, 0));
    }

    #[test]
    fn totals_add_what_was_measured_and_say_when_nothing_was() {
        let mut report = two_orgs();
        assert_eq!(report.summary().storage_bytes, None, "nothing measured");
        assert_eq!(report.orgs[0].storage_bytes(), None);

        report.orgs[0].apps[0].storage_bytes = Some(700);
        report.orgs[0].apps[0].storage_bytes_before = Some(500);
        report.orgs[1].apps[0].storage_bytes = Some(300);
        report.orgs[0].apps[0].current.function_calls = 40;
        report.orgs[0].apps[0].current.function_failures = 4;
        report.orgs[1].apps[0].current.releases = 2;
        let s = report.summary();
        // The unmeasured app is left out, not counted as empty.
        assert_eq!(
            (s.storage_bytes, s.storage_bytes_before),
            (Some(1000), Some(500))
        );
        assert_eq!(
            (s.function_calls, s.function_failures, s.releases),
            (40, 4, 2)
        );
        assert_eq!(report.orgs[0].storage_bytes(), Some(700));
        assert_eq!(report.orgs[1].releases(), 2);
    }

    #[test]
    fn a_release_alone_is_activity() {
        let shipped = Counts {
            releases: 1,
            ..Counts::default()
        };
        assert!(shipped.any_activity());
        assert!(!Counts::default().any_activity());
    }
}
