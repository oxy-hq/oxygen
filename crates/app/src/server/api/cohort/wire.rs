//! The cohort wire contract — shared verbatim by both surfaces.
//!
//! Owned here, next to the IDE handler, and imported by
//! [`crate::server::api::projects::cohort`], mirroring how
//! `api::metric_tree` owns `OpportunityRequest` and the customer-app twin
//! imports it. One definition means the two surfaces cannot drift into
//! answering the same question with different shapes.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use oxy_airlayer_compat::engine::cohort::{CohortSubject, ExcludedSubject, PeerCohortResult};
use oxy_airlayer_compat::engine::metric_tree_ops::BenchmarkStatistic;

/// Request body for a cohort resolution.
#[derive(Debug, Clone, Deserialize)]
pub struct CohortRequest {
    /// Entity the cohort is declared on, e.g. `"restaurant_id"`.
    pub entity: String,
    /// Fully-qualified measure to compare, e.g. `"sales.wage_pct"`.
    pub measure: String,
    /// Time dimension to filter the period on.
    pub time_dimension: String,
    /// `[start, end]`, inclusive, `YYYY-MM-DD`.
    pub period: (String, String),
    /// Cohort as `"entity.cohort_name"`. Falls back to the measure's
    /// `default_cohort:` when omitted.
    #[serde(default)]
    pub cohort: Option<String>,
    /// `median` (default) | `p75` | `best_peer`.
    #[serde(default)]
    pub statistic: Option<String>,
    /// The custom app asking, when there is one. Both siblings
    /// (`semantic_query.rs`, `world_model.rs`) pass it, and reach consults it
    /// for app-admin standing — omitting it silently narrows a per-app admin
    /// to their own roster.
    ///
    /// **The IDE handler ignores it**: that surface has no reach to narrow
    /// (see the [module docs](super)). Kept on one shared struct rather than
    /// split in two so the surfaces stay a single wire contract; an IDE caller
    /// simply omits the field.
    #[serde(default)]
    pub app: Option<Uuid>,
}

/// Oxy's mirror of airlayer's `PeerCohortResult`.
///
/// Deliberately a mirror rather than a re-serialization of the airlayer type:
/// airlayer is a git pin, and its `skip_serializing_if` attributes are "a
/// serde attribute today, not a guarantee". Owning the wire shape here means a
/// pin bump cannot silently drop a field the SDK and UI depend on, and lets
/// the contract be asserted by a test in this repo.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CohortResponse {
    pub entity: String,
    /// The cohort actually used — echoed so a caller that passed `None` and
    /// got the measure's `default_cohort:` can render which one answered.
    pub cohort: String,
    pub measure: String,
    pub statistic: BenchmarkStatistic,
    pub period: (String, String),
    /// The window the BAND was measured over. `Some(period)` when the cohort
    /// declares a band without its own `window:`; absent only when the cohort
    /// declares no band at all. Always contains `period` — it is anchored at
    /// the period START and extended backward, not shifted.
    ///
    /// Emitted as `null` rather than omitted: the field's absence is
    /// meaningful, so it must not be confusable with a pin-bump regression.
    pub band_window: Option<(String, String)>,
    pub subjects: Vec<CohortSubjectDto>,
    /// Subjects dropped before comparison, each with a reason. Disjoint from
    /// `subjects`. Never empty-by-omission — see the [module docs](super).
    pub excluded: Vec<ExcludedSubjectDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CohortSubjectDto {
    pub key: String,
    pub value: f64,
    /// `0.0` when `peer_count == 0` — there is no baseline, not a baseline of
    /// zero. Read it together with `peer_count`.
    pub baseline: f64,
    /// Oriented so positive always means opportunity, whichever direction the
    /// measure improves in.
    pub gap: f64,
    pub peers: Vec<String>,
    pub peer_count: usize,
    /// Whether `peer_count` met the cohort's `min_peers`. NOT a gate: an
    /// insufficient subject is still here, with its comparison computed.
    pub sufficient: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ExcludedSubjectDto {
    pub key: String,
    pub reason: String,
}

impl From<&CohortSubject> for CohortSubjectDto {
    fn from(s: &CohortSubject) -> Self {
        Self {
            key: s.key.clone(),
            value: s.value,
            baseline: s.baseline,
            gap: s.gap,
            peers: s.peers.clone(),
            peer_count: s.peer_count,
            sufficient: s.sufficient,
        }
    }
}

impl From<&ExcludedSubject> for ExcludedSubjectDto {
    fn from(e: &ExcludedSubject) -> Self {
        Self {
            key: e.key.clone(),
            reason: e.reason.clone(),
        }
    }
}

impl From<&PeerCohortResult> for CohortResponse {
    fn from(r: &PeerCohortResult) -> Self {
        Self {
            entity: r.entity.clone(),
            cohort: r.cohort.clone(),
            measure: r.measure.clone(),
            statistic: r.statistic,
            period: r.period.clone(),
            band_window: r.band_window.clone(),
            // Every subject, in pull order — no server-side filtering. See the
            // module docs: `sufficient` reports, it does not gate.
            subjects: r.subjects.iter().map(CohortSubjectDto::from).collect(),
            excluded: r.excluded.iter().map(ExcludedSubjectDto::from).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subject(key: &str, peer_count: usize, sufficient: bool) -> CohortSubject {
        CohortSubject {
            key: key.to_string(),
            value: 10.0,
            baseline: 8.0,
            gap: 2.0,
            peers: (0..peer_count).map(|i| format!("peer{i}")).collect(),
            peer_count,
            sufficient,
        }
    }

    fn result_with(
        subjects: Vec<CohortSubject>,
        excluded: Vec<ExcludedSubject>,
    ) -> PeerCohortResult {
        PeerCohortResult {
            entity: "restaurant_id".to_string(),
            cohort: "size_matched".to_string(),
            measure: "sales.wage_pct".to_string(),
            statistic: BenchmarkStatistic::Median,
            period: ("2026-08-01".to_string(), "2026-08-31".to_string()),
            band_window: Some(("2026-05-03".to_string(), "2026-08-31".to_string())),
            subjects,
            excluded,
        }
    }

    /// The reason the feature exists: a dropped subject must arrive with a
    /// reason, not vanish.
    #[test]
    fn excluded_subjects_survive_the_mirror_with_their_reasons() {
        let r = result_with(
            vec![subject("a", 4, true)],
            vec![
                ExcludedSubject {
                    key: "b".to_string(),
                    reason: "opened mid-period".to_string(),
                },
                ExcludedSubject {
                    key: "(null) [direct_only]".to_string(),
                    reason: "entity key was NULL".to_string(),
                },
            ],
        );
        let dto = CohortResponse::from(&r);
        assert_eq!(dto.excluded.len(), 2);
        assert_eq!(dto.excluded[0].key, "b");
        assert_eq!(dto.excluded[0].reason, "opened mid-period");
        // The synthesized id for a NULL key must survive verbatim — it is what
        // makes a keyless row addressable at all.
        assert_eq!(dto.excluded[1].key, "(null) [direct_only]");
    }

    /// `min_peers` is a reporting predicate. An insufficient subject is
    /// returned with its comparison computed; the server does not filter it.
    #[test]
    fn insufficient_subjects_are_returned_not_filtered() {
        let r = result_with(
            vec![subject("thin", 1, false), subject("thick", 9, true)],
            vec![],
        );
        let dto = CohortResponse::from(&r);
        assert_eq!(dto.subjects.len(), 2);
        let thin = &dto.subjects[0];
        assert_eq!(thin.key, "thin");
        assert!(!thin.sufficient);
        assert_eq!(thin.peer_count, 1);
        // Computed, not blanked: the comparison is there for the client to
        // judge.
        assert_eq!(thin.baseline, 8.0);
        assert_eq!(thin.gap, 2.0);
    }

    #[test]
    fn cohort_name_and_band_window_are_echoed_back() {
        let dto = CohortResponse::from(&result_with(vec![subject("a", 3, true)], vec![]));
        assert_eq!(dto.cohort, "size_matched");
        assert_eq!(
            dto.band_window,
            Some(("2026-05-03".to_string(), "2026-08-31".to_string()))
        );
    }

    /// An absent band window must serialize as an explicit `null`, never be
    /// omitted: a reader has to be able to tell "this cohort declares no band"
    /// from "a pin bump dropped the field".
    #[test]
    fn absent_band_window_serializes_as_null() {
        let mut r = result_with(vec![subject("a", 3, true)], vec![]);
        r.band_window = None;
        let json = serde_json::to_value(CohortResponse::from(&r)).unwrap();
        assert_eq!(
            json.get("band_window"),
            Some(&serde_json::Value::Null),
            "band_window must be present-and-null, not omitted: {json}"
        );
    }

    /// The wire value is the contract the SDK pins as a literal union; an
    /// upstream rename must fail here, not in a consumer.
    #[test]
    fn statistic_serializes_to_the_documented_wire_string() {
        let mut r = result_with(vec![], vec![]);
        let json = serde_json::to_value(CohortResponse::from(&r)).unwrap();
        assert_eq!(json["statistic"], "median");
        r.statistic = BenchmarkStatistic::P75;
        let json = serde_json::to_value(CohortResponse::from(&r)).unwrap();
        assert_eq!(json["statistic"], "p75");
        r.statistic = BenchmarkStatistic::BestPeer;
        let json = serde_json::to_value(CohortResponse::from(&r)).unwrap();
        assert_eq!(json["statistic"], "best_peer");
    }
}
