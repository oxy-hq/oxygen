//! The report as the console reads it: one reader's slice, with the totals and
//! highlights worked out for that slice.

use chrono::{DateTime, Utc};
use oxy_authz::Scope;
use serde::Serialize;
use uuid::Uuid;

use super::highlights::{Highlight, highlights};
use super::model::{AppUsage, OrgUsage, Summary};
use super::store::StoredReport;

#[derive(Debug, Serialize)]
pub struct ReportView {
    pub id: Uuid,
    /// The week covered: inclusive start, exclusive end, both a Monday 00:00 UTC.
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub generated_at: DateTime<Utc>,
    pub summary: Summary,
    pub highlights: Vec<Highlight>,
    pub orgs: Vec<OrgView>,
}

#[derive(Debug, Serialize)]
pub struct OrgView {
    pub org_id: Uuid,
    pub name: String,
    pub slug: String,
    pub people: u64,
    pub prev_people: u64,
    pub views: u64,
    pub prev_views: u64,
    pub function_calls: u64,
    pub function_failures: u64,
    /// Builds promoted to production this week.
    pub releases: u64,
    /// What the org's apps store; `null` when none was measured.
    pub storage_bytes: Option<u64>,
    pub apps: Vec<AppUsage>,
}

impl From<OrgUsage> for OrgView {
    fn from(org: OrgUsage) -> Self {
        Self {
            views: org.views(),
            prev_views: org.prev_views(),
            function_calls: org.function_calls(),
            function_failures: org.function_failures(),
            releases: org.releases(),
            storage_bytes: org.storage_bytes(),
            org_id: org.org_id,
            name: org.name,
            slug: org.slug,
            people: org.people,
            prev_people: org.prev_people,
            apps: org.apps,
        }
    }
}

impl ReportView {
    /// `report` as someone whose grant reaches `scope` may see it.
    pub fn of(report: &StoredReport, scope: &Scope) -> Self {
        let slice = report.snapshot.scoped(scope);
        Self {
            id: report.id,
            period_start: slice.period.start,
            period_end: slice.period.end,
            generated_at: report.created_at,
            summary: slice.summary(),
            highlights: highlights(&slice),
            orgs: slice.orgs.into_iter().map(OrgView::from).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::fixtures::*;
    use super::*;

    fn stored() -> StoredReport {
        StoredReport {
            id: Uuid::from_u128(99),
            created_at: period().end + chrono::Duration::hours(1),
            snapshot: snapshot(vec![
                org(1, "Poke House", vec![app(10, "Store Ops", 6, 4)]),
                org(2, "Rivermark", vec![app(20, "Crew Board", 0, 8)]),
            ]),
        }
    }

    #[test]
    fn the_view_carries_the_week_the_totals_and_the_highlights() {
        let view = ReportView::of(&stored(), &Scope::All);
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["period_start"], "2026-09-28T00:00:00Z");
        assert_eq!(json["period_end"], "2026-10-05T00:00:00Z");
        assert_eq!(json["summary"]["people"], 6);
        assert_eq!(json["summary"]["prev_people"], 12);
        assert_eq!(json["highlights"][0]["kind"], "went_quiet");
        assert_eq!(json["orgs"][0]["views"], 24);
        assert_eq!(json["orgs"][0]["releases"], 0);
        // Never measured reads as null, not as an empty app.
        assert!(json["orgs"][0]["storage_bytes"].is_null());
        assert!(json["summary"]["storage_bytes"].is_null());
        assert_eq!(json["highlights"][0]["detail"], "8 people → nobody");
        assert_eq!(json["orgs"][0]["apps"][0]["current"]["people"], 6);
    }

    #[test]
    fn a_bounded_reader_is_shown_nothing_about_an_org_out_of_reach() {
        let view = ReportView::of(&stored(), &Scope::Orgs(vec![Uuid::from_u128(1)]));
        let json = serde_json::to_string(&view).unwrap();
        assert!(json.contains("Poke House"));
        assert!(!json.contains("Rivermark"), "{json}");
        assert!(!json.contains("Crew Board"));
        // The totals and the highlights are the slice's, not the fleet's.
        assert_eq!(view.summary.prev_people, 4);
        assert!(view.highlights.is_empty());
    }
}
