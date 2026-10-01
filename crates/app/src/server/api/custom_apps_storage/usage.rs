//! Measuring an app's asset silo: the walk, and the rollup it produces.
//!
//! ## Why measure instead of index
//!
//! A per-object Postgres table was considered and rejected. Presigned uploads go
//! **direct to S3**, so oxy never observes the completion — a row minted at
//! presign time is a claim, not a fact, and reconciling it needs either S3 event
//! notifications (new infra) or exactly the walk below. A rollup computed *from*
//! the object store is correct by construction no matter who wrote the bytes, and
//! costs one LIST per 1,000 objects.
//!
//! ## The untagged number is derived, not read back
//!
//! `untagged_bytes` is computed by asking the app's **current** retention policy
//! whether each key would be tagged — not by calling `GetObjectTagging`, which
//! would be one request per object and dwarf the walk itself.
//!
//! So it answers "which bytes does today's policy fail to cover?", which is the
//! actionable question (*add a retention rule*), rather than "which objects
//! carry a tag right now". The two differ only for objects written before a
//! policy change, and in that gap the derived number is the more useful one: it
//! is what the silo will look like once those objects are rewritten.
//!
//! ## A partial walk is never rounded down
//!
//! Cutting the walk short and recording the smaller number would make a quota
//! silently fail open exactly when the object store is unhealthy. A truncated or
//! failed walk is recorded with `measure_status` set, and callers that enforce
//! anything are expected to look at it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{RetentionPolicy, StorageError};
use entity::app_storage_usage::measure_status;

/// Objects per LIST page. Page count is what the walk costs, so this asks for
/// the largest page the shared `list()` will serve — it is clamped to
/// `MAX_LIST_LIMIT`, which this deliberately matches rather than exceeds. Raising
/// it here alone would silently do nothing.
const MEASURE_PAGE_SIZE: usize = 1000;

/// Ceiling on pages for ONE app's walk, so a runaway silo can't monopolize the
/// sweeper. At 1,000 objects a page this covers 5M objects; past that the app is
/// recorded `partial` and should move to S3 Inventory (~80x cheaper than Storage
/// Lens for the same answer).
const MAX_MEASURE_PAGES: usize = 5_000;

/// Per-top-level-prefix split, captured during the walk we already pay for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrefixUsage {
    pub bytes: i64,
    pub objects: i64,
    /// The TTL class today's policy assigns this prefix (`"30d"`), or `None` for
    /// "kept forever" — what the UI renders as the per-prefix retention badge.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expire_after: Option<String>,
}

/// The result of one walk.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageMeasurement {
    pub bytes: i64,
    pub object_count: i64,
    pub untagged_bytes: i64,
    pub untagged_object_count: i64,
    /// Keyed by top-level segment with its trailing slash (`"uploads/"`), or
    /// `"(root)"` for objects sitting directly in the silo. An environment's
    /// silo is split the same way under its own `~<env>/` label
    /// (`"~staging/uploads/"`), so production's rows read as they always did
    /// and the total is still the sum of the rows.
    pub prefix_breakdown: BTreeMap<String, PrefixUsage>,
    pub status: &'static str,
    pub detail: Option<String>,
}

/// Objects with no leading path segment. Named rather than empty-string so the
/// UI has something to print and the JSON key is never `""`.
const ROOT_BUCKET: &str = "(root)";

impl UsageMeasurement {
    /// One object of production's silo (`environment: None`) or of an
    /// environment's.
    fn record_in(
        &mut self,
        environment: Option<&str>,
        relative_key: &str,
        size: i64,
        policy: &RetentionPolicy,
    ) {
        let label = environment.map(|env| format!("~{env}/"));
        self.record_labelled(label.as_deref().unwrap_or(""), relative_key, size, policy);
    }

    #[cfg(test)]
    fn record(&mut self, relative_key: &str, size: i64, policy: &RetentionPolicy) {
        self.record_labelled("", relative_key, size, policy);
    }

    fn record_labelled(
        &mut self,
        label: &str,
        relative_key: &str,
        size: i64,
        policy: &RetentionPolicy,
    ) {
        // A negative size is nonsense the object store should never emit; clamp
        // rather than let it silently subtract from a quota.
        let size = size.max(0);
        self.bytes += size;
        self.object_count += 1;

        let class = policy.resolve(relative_key);
        if class.is_none() {
            self.untagged_bytes += size;
            self.untagged_object_count += 1;
        }

        let bucket = match relative_key.split_once('/') {
            Some((head, _)) if !head.is_empty() => format!("{label}{head}/"),
            _ => format!("{label}{ROOT_BUCKET}"),
        };
        let entry = self.prefix_breakdown.entry(bucket).or_default();
        entry.bytes += size;
        entry.objects += 1;
        // Recorded per prefix from the first object seen under it. Uniform in
        // practice — the policy matches on exactly this prefix — and it saves
        // the UI re-deriving the policy to render a badge.
        if entry.expire_after.is_none() {
            entry.expire_after = class.map(|c| c.tag_value().to_string());
        }
    }

    pub fn prefix_breakdown_json(&self) -> serde_json::Value {
        serde_json::to_value(&self.prefix_breakdown).unwrap_or(serde_json::Value::Null)
    }

    /// Whether these numbers are exact. Anything else means they are a floor,
    /// and a quota decision built on them must not treat them as authoritative.
    pub fn is_exact(&self) -> bool {
        self.status == measure_status::OK
    }
}

/// Walk every silo of one app — production's and each environment's — and
/// total them. An environment's bytes are in the app's row and breakdown
/// under `~<env>/`, but `quota::production_bytes` excludes them from the org
/// total; each silo answers to its own cap (`environment_limits`) instead.
///
/// Never returns `Err` for a partial read: a walk that dies halfway still
/// carries real information (a floor on the size), and losing it entirely would
/// leave the rollup showing a stale number with no indication anything went
/// wrong. Errors surface through `status`/`detail` instead.
pub async fn measure_app(app_id: Uuid, policy: &RetentionPolicy) -> UsageMeasurement {
    let mut out = UsageMeasurement {
        status: measure_status::OK,
        ..Default::default()
    };
    let roots = match super::environments::silo_roots(app_id).await {
        Ok(roots) => roots,
        Err(e) => {
            out.status = measure_status::FAILED;
            out.detail = Some(describe(&e));
            return out;
        }
    };
    let mut pages_left = MAX_MEASURE_PAGES;
    for root in roots {
        if !walk_root(&mut out, app_id, &root, policy, &mut pages_left).await {
            return out;
        }
    }
    out
}

/// Walk one prefix into `out`, spending from the app's shared page budget.
/// `false` when the walk stopped short; `out` then says why.
async fn walk_root(
    out: &mut UsageMeasurement,
    app_id: Uuid,
    root: &str,
    policy: &RetentionPolicy,
    pages_left: &mut usize,
) -> bool {
    let mut cursor: Option<String> = None;
    loop {
        if *pages_left == 0 {
            out.status = measure_status::PARTIAL;
            out.detail = Some(format!(
                "walk stopped at the {MAX_MEASURE_PAGES}-page ceiling ({} objects); \
                 this app should move to an S3 Inventory-based measure",
                out.object_count
            ));
            return false;
        }
        *pages_left -= 1;
        let page = match super::ops::list_raw(root, MEASURE_PAGE_SIZE, cursor.clone()).await {
            Ok(p) => p,
            Err(e) => {
                // Distinguish "nothing measured" from "measured, then broke".
                // The first is a failure; the second is a usable floor.
                out.status = if out.object_count == 0 {
                    measure_status::FAILED
                } else {
                    measure_status::PARTIAL
                };
                out.detail = Some(describe(&e));
                return false;
            }
        };
        for object in &page.objects {
            let (environment, relative) = super::silo::split_silo_key(app_id, &object.key)
                .unwrap_or((None, object.key.as_str()));
            out.record_in(environment, relative, object.size, policy);
        }
        if !page.has_more {
            return true;
        }
        cursor = page.cursor;
        if cursor.is_none() {
            // `has_more` with no cursor would loop forever. Treat the
            // contradiction as a truncated walk rather than spinning.
            out.status = measure_status::PARTIAL;
            out.detail = Some("listing reported more pages but returned no cursor".to_string());
            return false;
        }
    }
}

fn describe(e: &StorageError) -> String {
    // Bounded: this lands in a DB column and an admin table cell, and an AWS SDK
    // error chain can run to kilobytes.
    let s = e.to_string();
    if s.len() > 500 {
        format!("{}…", &s[..500])
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::api::custom_apps_storage::RetentionRule;

    fn policy(rules: &[(&str, Option<&str>)]) -> RetentionPolicy {
        let rules: Vec<RetentionRule> = rules
            .iter()
            .map(|(prefix, expire_after)| RetentionRule {
                prefix: (*prefix).to_string(),
                expire_after: expire_after.map(str::to_string),
            })
            .collect();
        RetentionPolicy::from_rules(&rules).0
    }

    fn measure(entries: &[(&str, i64)], policy: &RetentionPolicy) -> UsageMeasurement {
        let mut m = UsageMeasurement {
            status: measure_status::OK,
            ..Default::default()
        };
        for (key, size) in entries {
            m.record(key, *size, policy);
        }
        m
    }

    #[test]
    fn totals_bytes_and_objects() {
        let m = measure(
            &[("uploads/a.png", 100), ("uploads/b.png", 250)],
            &RetentionPolicy::default(),
        );
        assert_eq!(m.bytes, 350);
        assert_eq!(m.object_count, 2);
    }

    #[test]
    fn untagged_counts_only_what_no_rule_covers() {
        let p = policy(&[("generated/", Some("30d"))]);
        let m = measure(
            &[
                ("generated/report.pdf", 1000), // covered
                ("uploads/scan.png", 40),       // not covered
                ("stray.bin", 2),               // not covered
            ],
            &p,
        );
        assert_eq!(m.bytes, 1042);
        // The signal that matters: 42 bytes nothing will ever reclaim.
        assert_eq!(m.untagged_bytes, 42);
        assert_eq!(m.untagged_object_count, 2);
    }

    #[test]
    fn a_pinned_prefix_counts_as_untagged() {
        // `expireAfter: null` is a deliberate "keep forever" — and it is still
        // unbounded growth, so the operator-facing number must include it.
        // Anything else would hide the largest silos precisely because someone
        // chose to keep them.
        let p = policy(&[("uploads/", None)]);
        let m = measure(&[("uploads/keep.bin", 500)], &p);
        assert_eq!(m.untagged_bytes, 500);
    }

    #[test]
    fn prefix_breakdown_splits_on_the_top_level_segment() {
        let p = policy(&[("generated/", Some("90d"))]);
        let m = measure(
            &[
                ("generated/2026/q1.pdf", 10),
                ("generated/2026/q2.pdf", 20),
                ("uploads/a.png", 5),
            ],
            &p,
        );
        let generated = &m.prefix_breakdown["generated/"];
        assert_eq!((generated.bytes, generated.objects), (30, 2));
        // Nested paths roll up to their TOP-level segment, not each directory.
        assert_eq!(generated.expire_after.as_deref(), Some("90d"));
        let uploads = &m.prefix_breakdown["uploads/"];
        assert_eq!((uploads.bytes, uploads.objects), (5, 1));
        assert_eq!(uploads.expire_after, None);
    }

    #[test]
    fn root_level_objects_get_their_own_bucket() {
        let m = measure(&[("loose.bin", 7)], &RetentionPolicy::default());
        assert_eq!(m.prefix_breakdown[ROOT_BUCKET].bytes, 7);
        // Never an empty-string JSON key.
        assert!(!m.prefix_breakdown.contains_key(""));
    }

    #[test]
    fn a_negative_size_cannot_subtract_from_the_total() {
        let m = measure(
            &[("uploads/a", -5), ("uploads/b", 10)],
            &RetentionPolicy::default(),
        );
        assert_eq!(m.bytes, 10);
        assert_eq!(m.object_count, 2);
    }

    #[test]
    fn is_exact_is_false_for_a_partial_walk() {
        let mut m = measure(&[("uploads/a", 1)], &RetentionPolicy::default());
        assert!(m.is_exact());
        m.status = measure_status::PARTIAL;
        assert!(!m.is_exact());
        m.status = measure_status::FAILED;
        assert!(!m.is_exact());
    }

    #[test]
    fn an_environment_silo_counts_toward_the_total_under_its_own_label() {
        let p = policy(&[("generated/", Some("90d"))]);
        let mut m = measure(&[("generated/a.pdf", 10)], &p);
        m.record_in(Some("staging"), "generated/b.pdf", 5, &p);
        m.record_in(Some("staging"), "loose.bin", 1, &p);
        assert_eq!(
            (m.bytes, m.object_count),
            (16, 3),
            "staging's bytes are the app's"
        );
        assert_eq!(m.untagged_bytes, 1);
        assert_eq!(m.prefix_breakdown["generated/"].bytes, 10);
        let staging = &m.prefix_breakdown["~staging/generated/"];
        assert_eq!(staging.bytes, 5);
        assert_eq!(
            staging.expire_after.as_deref(),
            Some("90d"),
            "the policy reads the relative key"
        );
        assert_eq!(m.prefix_breakdown["~staging/(root)"].bytes, 1);
        let rows: i64 = m.prefix_breakdown.values().map(|r| r.bytes).sum();
        assert_eq!(rows, m.bytes, "the rows still sum to the total");
    }

    #[test]
    fn breakdown_serializes_without_the_none_class() {
        let p = policy(&[("generated/", Some("90d"))]);
        let m = measure(&[("generated/a.pdf", 1), ("uploads/b.png", 1)], &p);
        let json = m.prefix_breakdown_json();
        assert_eq!(json["generated/"]["expireAfter"], "90d");
        // Omitted rather than null, so the UI can branch on presence.
        assert!(json["uploads/"].get("expireAfter").is_none());
    }
}
