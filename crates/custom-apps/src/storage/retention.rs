//! Retention for the custom-app asset store: TTL classes, the object tag that
//! carries one, and the bucket lifecycle rules that act on it.
//!
//! ## Why tags and not prefixes
//!
//! The obvious shape — one lifecycle rule per app prefix — does not scale. S3
//! caps a bucket at **1,000 lifecycle rules** and this bucket is shared by every
//! app, so at three prefix-classes per app it ceilings out around 330 apps and
//! then starts silently failing to protect new ones.
//!
//! So the rule set is **fixed and tiny**: one rule per TTL class, keyed on the
//! `oxy-ttl` object tag. Five rules, forever, no matter how many apps exist. Oxy
//! stamps the tag at write time from the app's declared policy; S3 does the
//! deleting, for free.
//!
//! ## This module does not apply anything
//!
//! **Terraform writes those rules; this module verifies them.** Oxy is still the
//! source of truth for *what* they must be — it stamps the tags they filter on —
//! but the infrastructure repo owns the bucket's lifecycle configuration, because
//! `PutBucketLifecycleConfiguration` replaces the whole thing and two
//! authoritative writers would delete each other's work forever. The reasoning is
//! in full beside [`verify_lifecycle_rules`].
//!
//! ## Fail open, always
//!
//! An object with no `oxy-ttl` tag **never expires**. Every path here that cannot
//! resolve a class — no manifest, an unparseable one, a prefix that matches
//! nothing, a class name we don't recognize — yields `None` and the object lives
//! forever. Deleting an author's data because a manifest line was missing or
//! misspelled is far worse than keeping bytes we could have reclaimed.
//!
//! ## Three behaviours that read as bugs but are not
//!
//! * **Expiry is not exact.** S3 evaluates tag-filtered rules daily, so an object
//!   tagged `1d` disappears *within about a day* of its first birthday, not on the
//!   hour.
//! * **Removing the tag cancels a pending delete.** S3 re-checks the tag when the
//!   queued action executes. That is how "pin this object" works, with no extra
//!   machinery.
//! * **Editing a manifest rule does not retag what is already stored.** New writes
//!   get the new class; existing objects keep theirs until something rewrites them.

use aws_sdk_s3::types::{ExpirationStatus, LifecycleRule, Tag};
use serde::{Deserialize, Serialize};

use super::StorageError;

/// Tag key stamped on every object that carries a retention class.
pub const TTL_TAG_KEY: &str = "oxy-ttl";

// Incomplete-multipart cleanup is NOT verified here. It is a bucket-policy
// concern with no tag the app stamps and no contract the app can state, so it
// belongs entirely to Terraform (which has carried an
// `abort-incomplete-multipart-uploads` rule on this bucket since before any of
// this existed). Verifying it would mean asserting infra policy from application
// code.

// ── TTL classes ──────────────────────────────────────────────────────────────

/// The fixed set of retention classes an app may ask for.
///
/// Deliberately closed. A free-form duration (`"37d"`) would need its own
/// lifecycle rule, which reintroduces exactly the per-app rule explosion the tag
/// scheme exists to avoid — so an unrecognized value is rejected at parse time
/// rather than silently rounded to a neighbour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtlClass {
    D1,
    D7,
    D30,
    D90,
    D365,
}

impl TtlClass {
    /// Every class, in ascending order. The single source the rule set and the
    /// parser both read, so they cannot drift apart.
    pub const ALL: [TtlClass; 5] = [
        TtlClass::D1,
        TtlClass::D7,
        TtlClass::D30,
        TtlClass::D90,
        TtlClass::D365,
    ];

    /// Parse a manifest value (`"30d"`). Case- and whitespace-tolerant; anything
    /// else is `None`, which the caller turns into a warning and no tag.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "1d" => Some(TtlClass::D1),
            "7d" => Some(TtlClass::D7),
            "30d" => Some(TtlClass::D30),
            "90d" => Some(TtlClass::D90),
            "365d" => Some(TtlClass::D365),
            _ => None,
        }
    }

    /// The tag value stored on the object, and the value the lifecycle rule
    /// filters on. These must be the same string — hence one function.
    pub fn tag_value(self) -> &'static str {
        match self {
            TtlClass::D1 => "1d",
            TtlClass::D7 => "7d",
            TtlClass::D30 => "30d",
            TtlClass::D90 => "90d",
            TtlClass::D365 => "365d",
        }
    }

    fn days(self) -> i32 {
        match self {
            TtlClass::D1 => 1,
            TtlClass::D7 => 7,
            TtlClass::D30 => 30,
            TtlClass::D90 => 90,
            TtlClass::D365 => 365,
        }
    }

    /// The `x-amz-tagging` header value for an object of this class. S3 wants
    /// URL-query form; every byte we emit here is `[a-z0-9-]`, so there is
    /// nothing to percent-encode and nothing a caller can inject — the class set
    /// is closed and the key is a constant.
    pub fn tagging_header(self) -> String {
        format!("{TTL_TAG_KEY}={}", self.tag_value())
    }
}

// ── Manifest policy ──────────────────────────────────────────────────────────

/// One `storage.retention[]` entry from `oxy-app.json`.
///
/// `expireAfter: null` is meaningful and distinct from an absent rule: it pins a
/// prefix to "keep forever" so a broader sibling rule can't sweep it up.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetentionRule {
    pub prefix: String,
    #[serde(default)]
    pub expire_after: Option<String>,
}

/// An app's resolved retention policy — the manifest rules, validated and sorted
/// so the longest prefix wins.
#[derive(Debug, Clone, Default)]
pub struct RetentionPolicy {
    /// `(prefix, class)`, longest prefix first. `None` = keep forever.
    rules: Vec<(String, Option<TtlClass>)>,
}

impl RetentionPolicy {
    /// Build a policy from manifest rules. Returns the policy plus human-readable
    /// warnings for entries that were dropped.
    ///
    /// Bad entries are **skipped, not fatal**. A typo in one retention line must
    /// not take down every storage write the app makes — and since a skipped rule
    /// means "no tag", the failure mode is keeping bytes forever, which is the
    /// safe direction.
    pub fn from_rules(raw: &[RetentionRule]) -> (Self, Vec<String>) {
        let mut rules: Vec<(String, Option<TtlClass>)> = Vec::new();
        let mut warnings = Vec::new();

        for rule in raw {
            let prefix = rule.prefix.trim().trim_start_matches('/').to_string();
            if prefix.is_empty() {
                warnings.push("retention rule with an empty prefix was ignored".to_string());
                continue;
            }
            let class = match rule.expire_after.as_deref() {
                // Explicit "keep forever".
                None => None,
                Some(raw_class) => match TtlClass::parse(raw_class) {
                    Some(c) => Some(c),
                    None => {
                        warnings.push(format!(
                            "retention rule for '{prefix}' asks for expireAfter '{raw_class}', \
                             which is not one of {}; the prefix will not expire",
                            Self::supported_classes()
                        ));
                        continue;
                    }
                },
            };
            if rules.iter().any(|(p, _)| p == &prefix) {
                warnings.push(format!(
                    "duplicate retention rule for prefix '{prefix}' was ignored; \
                     the first one wins"
                ));
                continue;
            }
            rules.push((prefix, class));
        }

        // Longest prefix first, so `resolve` can return on the first hit and
        // `generated/tmp/` beats `generated/`.
        rules.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
        (Self { rules }, warnings)
    }

    fn supported_classes() -> String {
        TtlClass::ALL
            .iter()
            .map(|c| c.tag_value())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Resolve the class for a key **relative to the app silo** (`uploads/x.png`,
    /// never the full `customer-app-storage/<id>/uploads/x.png`).
    ///
    /// Longest matching prefix wins. No match → `None` → no tag → never expires.
    pub fn resolve(&self, relative_key: &str) -> Option<TtlClass> {
        let key = relative_key.trim_start_matches('/');
        self.rules
            .iter()
            .find(|(prefix, _)| key.starts_with(prefix.as_str()))
            .and_then(|(_, class)| *class)
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

// ── Bucket lifecycle rules: verified here, OWNED by Terraform ────────────────
//
// This module used to WRITE the rules at boot. It no longer does, because the
// infrastructure repo already manages `aws_s3_bucket_lifecycle_configuration` on
// this bucket (incomplete-MPU cleanup, noncurrent-version reaping), and
// `PutBucketLifecycleConfiguration` replaces the WHOLE configuration with no
// per-rule upsert. Two authoritative writers on one resource is a drift war:
// `terraform apply` deletes the app's rules, the next pod restart puts them back,
// and every plan shows drift forever.
//
// So ownership went one way — Terraform writes, the app verifies — and the
// contract that used to be implicit is now checked at runtime. The app is still
// the source of truth for WHAT the rules must be, because it stamps the tags they
// filter on; it just no longer applies them.
//
// The check is **semantic, not structural**: it asks "is there an enabled rule
// that expires objects tagged `oxy-ttl=<class>` after <class> days", not "is there
// a rule with this id and byte-identical fields". Terraform's generated rules
// differ from the SDK's structs in optional fields and naming, and pinning those
// would fail on cosmetic differences while missing real ones.

/// One class's expiry contract, as the verifier checks it.
#[derive(Debug, PartialEq, Eq)]
pub enum ClassStatus {
    /// An enabled rule expires this tag at the expected age.
    Satisfied,
    /// A rule filters this tag but expires at the wrong age, or is disabled.
    Mismatched {
        found_days: Option<i32>,
        enabled: bool,
    },
    /// A rule filters this tag but narrows the selection further, so not every
    /// object of the class expires. Reported separately from
    /// [`ClassStatus::Missing`] because the message for that one — "no rule
    /// expires this tag" — is false here in both halves, and an operator who
    /// greps the Terraform, finds the rule, and reads "it does not exist"
    /// concludes the verifier is broken.
    ///
    /// Only the **first** narrowing rule found for a class is reported, not all
    /// of them. Like the sweeper's `incomplete` flag, the log is a floor on the
    /// work: fixing the named rule and re-running surfaces the next one. Naming
    /// one rule an operator can act on beats a list they have to triage.
    Narrowed {
        rule_id: Option<String>,
        reason: &'static str,
    },
    /// Nothing on the bucket filters this tag — objects of this class never expire.
    Missing,
}

/// The outcome of one verification pass.
#[derive(Debug, PartialEq, Eq)]
pub struct VerifyReport {
    pub classes: Vec<(TtlClass, ClassStatus)>,
}

impl VerifyReport {
    pub fn is_satisfied(&self) -> bool {
        self.classes
            .iter()
            .all(|(_, s)| *s == ClassStatus::Satisfied)
    }

    /// Human-readable list of what is wrong, for the boot log.
    pub fn problems(&self) -> Vec<String> {
        self.classes
            .iter()
            .filter_map(|(class, status)| match status {
                ClassStatus::Satisfied => None,
                ClassStatus::Missing => Some(format!(
                    "no rule expires `{}={}` — objects of that class never expire",
                    TTL_TAG_KEY,
                    class.tag_value()
                )),
                ClassStatus::Narrowed { rule_id, reason } => Some(format!(
                    "rule {} filters `{}={}` but {reason}, so not every object of that \
                     class expires",
                    rule_id.as_deref().unwrap_or("<unnamed>"),
                    TTL_TAG_KEY,
                    class.tag_value()
                )),
                ClassStatus::Mismatched { found_days, enabled } => Some(format!(
                    "`{}={}` should expire after {} days but the bucket says {} (enabled: {enabled})",
                    TTL_TAG_KEY,
                    class.tag_value(),
                    class.days(),
                    found_days.map_or(
                        // `None` also covers a date-based expiration, which does
                        // expire — just not on a day count this can compare.
                        "no day-based expiration".to_string(),
                        |d| d.to_string()
                    ),
                )),
            })
            .collect()
    }
}

/// How a rule relates to class `value`.
#[derive(Debug, PartialEq, Eq)]
enum TagMatch {
    /// Selects every object of the class.
    Covers,
    /// Selects the tag but a strict subset of it; the reason is operator-facing.
    Narrowed(&'static str),
    /// Says nothing about this class.
    NoMatch,
}

/// Classify a rule against one TTL class.
///
/// Deliberately permissive about SHAPE and strict about SCOPE, and it reports
/// which — a rule that narrows is a different operator problem from one that is
/// absent, and saying "absent" about a rule sitting in the Terraform is how a
/// verifier gets written off as broken.
///
/// **Accepted** (all select exactly the class): the tag directly on the filter,
/// or inside an `and` block — which is what Terraform emits whenever a filter
/// carries more than a bare prefix — optionally with a prefix the silo sits
/// inside. Every key this app writes lives under `customer-app-storage/`, so
/// scoping a rule to that prefix on a shared bucket is the obvious thing to do.
///
/// **Narrowed** (some of the class never expires): a second tag, a size bound, or
/// a prefix that does not cover the silo root — deeper than it, or disjoint from
/// it entirely, in which case the rule selects nothing here at all.
fn match_ttl_tag(rule: &LifecycleRule, value: &str) -> TagMatch {
    let Some(filter) = rule.filter() else {
        return TagMatch::NoMatch;
    };
    let matches = |t: &Tag| t.key() == TTL_TAG_KEY && t.value() == value;

    // Defensive only: S3's `Filter` is a union — exactly one of Prefix, Tag,
    // ObjectSizeGreaterThan, ObjectSizeLessThan or And — so a size bound
    // alongside a tag always arrives inside `And`, where the check below catches
    // it. This branch cannot fire against anything S3 actually returns; it is
    // here so a future SDK relaxing that shape doesn't silently pass.
    if filter.object_size_greater_than().is_some() || filter.object_size_less_than().is_some() {
        return TagMatch::NoMatch;
    }

    // Same union rule means a bare `tag` filter carries no prefix, so there is
    // nothing to check here that the `and` branch checks below.
    if filter.tag().is_some_and(matches) {
        return TagMatch::Covers;
    }

    let Some(and) = filter.and() else {
        return TagMatch::NoMatch;
    };
    if !and.tags().iter().any(matches) {
        return TagMatch::NoMatch;
    }
    if and.tags().len() > 1 {
        return TagMatch::Narrowed("also requires another tag");
    }
    if and.object_size_greater_than().is_some() || and.object_size_less_than().is_some() {
        return TagMatch::Narrowed("is bounded by object size");
    }
    if !and.prefix().is_none_or(prefix_covers_silo) {
        // Covers both a prefix DEEPER than the silo (part of the class expires)
        // and one DISJOINT from it (none of it does — `logs/` selects nothing
        // here). "narrower than" would be generous by exactly one part in the
        // disjoint case, so the wording states only what is true of both.
        return TagMatch::Narrowed("is scoped to a prefix that does not cover the asset silo");
    }
    TagMatch::Covers
}

/// The key prefix every object in this store sits under. Kept honest against
/// [`super::app_prefix`] by `prefix_covers_silo_tracks_app_prefix`.
const SILO_ROOT: &str = "customer-app-storage/";

/// Does this rule prefix still cover every key the app writes?
///
/// True for the empty prefix, for the silo root, and for any prefix the silo root
/// starts with (`customer-app-` selects a superset). False for anything deeper —
/// `customer-app-storage/uploads/` would leave `generated/` unexpiring.
fn prefix_covers_silo(prefix: &str) -> bool {
    prefix.is_empty() || SILO_ROOT.starts_with(prefix)
}

/// Check the bucket's lifecycle configuration against the classes this code
/// stamps. Read-only — needs `s3:GetLifecycleConfiguration` and nothing more.
pub async fn verify_lifecycle_rules(bucket: &str) -> Result<VerifyReport, StorageError> {
    let client = super::s3::client().await;

    let existing: Vec<LifecycleRule> = match client
        .get_bucket_lifecycle_configuration()
        .bucket(bucket)
        .send()
        .await
    {
        Ok(out) => out.rules().to_vec(),
        Err(e) => {
            let detail = format!("{e}");
            // An absent configuration is not an error to fetch — it is a bucket
            // with no rules at all, which the report below describes as every
            // class missing.
            if is_no_such_lifecycle_configuration(&detail) {
                Vec::new()
            } else {
                return Err(StorageError::S3(format!(
                    "get_bucket_lifecycle_configuration {bucket}: {e} \
                     (the role needs s3:GetLifecycleConfiguration)"
                )));
            }
        }
    };

    Ok(VerifyReport {
        classes: TtlClass::ALL
            .iter()
            .map(|&class| (class, classify_rule(&existing, class)))
            .collect(),
    })
}

/// Split out so the matching logic is testable without S3.
pub fn classify_rule(rules: &[LifecycleRule], class: TtlClass) -> ClassStatus {
    let mut covering: Vec<&LifecycleRule> = Vec::new();
    let mut narrowed: Option<(&LifecycleRule, &'static str)> = None;
    for rule in rules {
        match match_ttl_tag(rule, class.tag_value()) {
            TagMatch::Covers => covering.push(rule),
            TagMatch::Narrowed(reason) => {
                // First one wins: it is the witness for the diagnostic. The
                // report names one rule per class, not every narrowing rule —
                // the boot log is a floor on the work, so an operator who fixes
                // the named rule and re-runs sees the next one.
                narrowed.get_or_insert((rule, reason));
            }
            TagMatch::NoMatch => continue,
        };
    }

    // ANY satisfying rule is enough — S3 applies them all, so one correct rule
    // expires the class whatever sits beside it. Taking the FIRST match instead
    // would report a stale disabled rule ordered ahead of the live one as a
    // mismatch, which is exactly the state a bucket passes through during a
    // Terraform migration that adds a replacement before removing the original.
    if covering.iter().any(|r| satisfies(r, class)) {
        return ClassStatus::Satisfied;
    }
    if let Some(first) = covering.first() {
        // Right scope, wrong terms.
        return ClassStatus::Mismatched {
            found_days: first.expiration().and_then(|e| e.days()),
            enabled: *first.status() == ExpirationStatus::Enabled,
        };
    }
    // Nothing covers the class. A rule that mentions the tag but narrows it is a
    // different problem from nothing at all, and the operator needs to know which.
    if let Some((rule, reason)) = narrowed {
        return ClassStatus::Narrowed {
            rule_id: rule.id().map(str::to_string),
            reason,
        };
    }
    ClassStatus::Missing
}

fn satisfies(rule: &LifecycleRule, class: TtlClass) -> bool {
    *rule.status() == ExpirationStatus::Enabled
        && rule.expiration().and_then(|e| e.days()) == Some(class.days())
}

/// S3 reports "this bucket has no lifecycle configuration" as an unmodeled error,
/// so it has to be matched on the wire signal — the same shape as the 412 check
/// in `s3::is_precondition_failed`.
fn is_no_such_lifecycle_configuration(detail: &str) -> bool {
    detail.contains("NoSuchLifecycleConfiguration")
}

/// The `fleet_job_claims` row the check claims once per build.
pub const LIFECYCLE_VERIFY_JOB: &str = "custom_app_asset_lifecycle_verify";

/// Verify the asset bucket's retention rules once per build, in the background.
///
/// Logs and never fails: a missing rule means bytes accumulate, not that anything
/// is broken right now, and taking the process down over it would turn a cost
/// problem into an availability one. It is logged at ERROR because the symptom —
/// storage that quietly never shrinks — is otherwise invisible until the bill.
///
/// Once per build across the fleet: every replica that runs singleton work
/// starts it, and the [`fleet_claim`](oxy_app_core::fleet_claim) row, keyed by
/// this build's version, lets the first of them make the call. The answer is
/// the same from every replica; the TTL classes it checks change with the build.
pub fn spawn_lifecycle_verify(db: sea_orm::DatabaseConnection) {
    let Some(bucket) = super::bucket() else {
        tracing::info!(
            "custom-app asset retention: no OXY_CUSTOMER_APPS_STORAGE_S3_BUCKET, so there \
             are no lifecycle rules to verify and local assets never expire (point \
             AWS_ENDPOINT_URL at MinIO to exercise them)"
        );
        return;
    };
    tokio::spawn(async move {
        // The version alone repeats across dev images built from one release;
        // the commit tells those apart where CI stamped it.
        let build = format!(
            "{}+{}",
            env!("CARGO_PKG_VERSION"),
            option_env!("GITHUB_SHA").unwrap_or("unknown")
        );
        match oxy_app_core::fleet_claim::claim_once(&db, LIFECYCLE_VERIFY_JOB, &build).await {
            Ok(true) => {}
            Ok(false) => {
                tracing::debug!(%build, "custom-app asset retention: already verified for this build");
                return;
            }
            // Verify anyway: an extra read-only call beats an unverified bucket.
            Err(e) => {
                tracing::warn!(error = %e, "custom-app asset retention: claim failed; verifying anyway")
            }
        }
        match verify_lifecycle_rules(&bucket).await {
            Ok(report) if report.is_satisfied() => tracing::info!(
                "custom-app asset retention: all {} TTL classes have a matching lifecycle \
                 rule on '{bucket}'",
                TtlClass::ALL.len()
            ),
            Ok(report) => tracing::error!(
                "custom-app asset retention: '{bucket}' does not enforce every TTL class \
                 this build stamps, so tagged objects will NOT expire. Terraform owns these \
                 rules (infrastructure: terraform/customer-apps-storage.tf) — fix them there. \
                 Problems: {}",
                report.problems().join("; ")
            ),
            Err(e) => tracing::error!(
                "custom-app asset retention: could not read lifecycle rules on '{bucket}' \
                 ({e}); retention is unverified"
            ),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    // Rule builders are test-only now — production code only READS rules.
    use aws_sdk_s3::types::{LifecycleExpiration, LifecycleRuleAndOperator, LifecycleRuleFilter};

    fn rule(prefix: &str, expire_after: Option<&str>) -> RetentionRule {
        RetentionRule {
            prefix: prefix.to_string(),
            expire_after: expire_after.map(str::to_string),
        }
    }

    // ── TTL classes ──────────────────────────────────────────────────────────

    #[test]
    fn class_parse_is_tolerant_of_case_and_whitespace() {
        assert_eq!(TtlClass::parse(" 30D "), Some(TtlClass::D30));
        assert_eq!(TtlClass::parse("365d"), Some(TtlClass::D365));
    }

    #[test]
    fn an_unsupported_duration_is_rejected_not_rounded() {
        // Rounding "37d" to 30d would silently delete data a week early.
        assert_eq!(TtlClass::parse("37d"), None);
        assert_eq!(TtlClass::parse("forever"), None);
        assert_eq!(TtlClass::parse(""), None);
    }

    #[test]
    fn the_tag_a_writer_stamps_is_the_one_the_verifier_looks_for() {
        // These are the two halves of the contract with Terraform: the value
        // stamped on the object and the value the rule must filter on. If they
        // ever diverge, every object silently stops expiring.
        for class in TtlClass::ALL {
            assert_eq!(
                class.tagging_header(),
                format!("{TTL_TAG_KEY}={}", class.tag_value())
            );
        }
    }

    // ── Policy resolution ────────────────────────────────────────────────────

    #[test]
    fn longest_prefix_wins() {
        let (policy, warnings) = RetentionPolicy::from_rules(&[
            rule("generated/", Some("90d")),
            rule("generated/tmp/", Some("1d")),
        ]);
        assert!(warnings.is_empty());
        assert_eq!(
            policy.resolve("generated/tmp/scratch.csv"),
            Some(TtlClass::D1)
        );
        assert_eq!(policy.resolve("generated/q1.pdf"), Some(TtlClass::D90));
    }

    #[test]
    fn an_unmatched_key_never_expires() {
        let (policy, _) = RetentionPolicy::from_rules(&[rule("tmp/", Some("1d"))]);
        assert_eq!(policy.resolve("uploads/invoice.pdf"), None);
    }

    #[test]
    fn explicit_null_pins_a_prefix_against_a_broader_rule() {
        // `uploads/` expires, but `uploads/permanent/` is pinned forever.
        let (policy, _) = RetentionPolicy::from_rules(&[
            rule("uploads/", Some("30d")),
            rule("uploads/permanent/", None),
        ]);
        assert_eq!(policy.resolve("uploads/scan.png"), Some(TtlClass::D30));
        assert_eq!(policy.resolve("uploads/permanent/deed.pdf"), None);
    }

    #[test]
    fn a_bad_class_warns_and_leaves_the_prefix_unexpiring() {
        let (policy, warnings) = RetentionPolicy::from_rules(&[rule("tmp/", Some("2 weeks"))]);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("2 weeks"), "{}", warnings[0]);
        // Fail open: the typo must not delete anything.
        assert_eq!(policy.resolve("tmp/x"), None);
    }

    #[test]
    fn an_empty_prefix_is_rejected_rather_than_matching_everything() {
        let (policy, warnings) = RetentionPolicy::from_rules(&[rule("  ", Some("1d"))]);
        assert_eq!(warnings.len(), 1);
        assert!(policy.is_empty());
        assert_eq!(policy.resolve("anything"), None);
    }

    #[test]
    fn a_leading_slash_still_matches() {
        let (policy, _) = RetentionPolicy::from_rules(&[rule("/tmp/", Some("1d"))]);
        assert_eq!(policy.resolve("tmp/x"), Some(TtlClass::D1));
        assert_eq!(policy.resolve("/tmp/x"), Some(TtlClass::D1));
    }

    #[test]
    fn a_duplicate_prefix_warns_and_keeps_the_first() {
        let (policy, warnings) =
            RetentionPolicy::from_rules(&[rule("tmp/", Some("1d")), rule("tmp/", Some("365d"))]);
        assert_eq!(warnings.len(), 1);
        assert_eq!(policy.resolve("tmp/x"), Some(TtlClass::D1));
    }

    #[test]
    fn an_absent_policy_never_expires_anything() {
        let policy = RetentionPolicy::default();
        assert!(policy.is_empty());
        assert_eq!(policy.resolve("uploads/anything.bin"), None);
    }

    // ── Verification ─────────────────────────────────────────────────────────

    fn ttl_rule(value: &str, days: i32, enabled: bool) -> LifecycleRule {
        LifecycleRule::builder()
            .id(format!("expire-{value}"))
            .status(if enabled {
                ExpirationStatus::Enabled
            } else {
                ExpirationStatus::Disabled
            })
            .filter(
                LifecycleRuleFilter::builder()
                    .tag(
                        Tag::builder()
                            .key(TTL_TAG_KEY)
                            .value(value)
                            .build()
                            .unwrap(),
                    )
                    .build(),
            )
            .expiration(LifecycleExpiration::builder().days(days).build())
            .build()
            .expect("rule")
    }

    #[test]
    fn a_matching_rule_satisfies_its_class() {
        let rules = vec![ttl_rule("30d", 30, true)];
        assert_eq!(classify_rule(&rules, TtlClass::D30), ClassStatus::Satisfied);
    }

    #[test]
    fn an_absent_rule_is_missing_not_satisfied() {
        // The failure that costs money: nothing filters the tag, so objects of
        // that class accumulate forever while everything looks fine.
        assert_eq!(classify_rule(&[], TtlClass::D30), ClassStatus::Missing);
        let unrelated = vec![ttl_rule("7d", 7, true)];
        assert_eq!(
            classify_rule(&unrelated, TtlClass::D30),
            ClassStatus::Missing
        );
    }

    #[test]
    fn a_wrong_day_count_is_a_mismatch_not_a_pass() {
        // Terraform saying 60 where the code stamps 30 is exactly the drift this
        // verifier exists to catch — it must not round to "close enough".
        let rules = vec![ttl_rule("30d", 60, true)];
        assert_eq!(
            classify_rule(&rules, TtlClass::D30),
            ClassStatus::Mismatched {
                found_days: Some(60),
                enabled: true
            }
        );
    }

    #[test]
    fn a_disabled_rule_does_not_satisfy_its_class() {
        // A disabled rule reads as configured but expires nothing.
        let rules = vec![ttl_rule("30d", 30, false)];
        assert_eq!(
            classify_rule(&rules, TtlClass::D30),
            ClassStatus::Mismatched {
                found_days: Some(30),
                enabled: false
            }
        );
    }

    #[test]
    fn a_tag_inside_an_and_block_still_counts() {
        // Terraform emits `filter { and { tags = {...} } }` for a single-tag
        // filter. Rejecting that shape would report a correctly-configured
        // bucket as broken — a false alarm that trains people to ignore the log.
        let rule = LifecycleRule::builder()
            .id("tf-style")
            .status(ExpirationStatus::Enabled)
            .filter(
                LifecycleRuleFilter::builder()
                    .and(
                        LifecycleRuleAndOperator::builder()
                            .tags(
                                Tag::builder()
                                    .key(TTL_TAG_KEY)
                                    .value("90d")
                                    .build()
                                    .unwrap(),
                            )
                            .build(),
                    )
                    .build(),
            )
            .expiration(LifecycleExpiration::builder().days(90).build())
            .build()
            .expect("rule");
        assert_eq!(
            classify_rule(&[rule], TtlClass::D90),
            ClassStatus::Satisfied
        );
    }

    fn and_rule(
        value: &str,
        days: i32,
        prefix: Option<&str>,
        extra_tag: bool,
        size_floor: Option<i64>,
    ) -> LifecycleRule {
        let mut and = LifecycleRuleAndOperator::builder().tags(
            Tag::builder()
                .key(TTL_TAG_KEY)
                .value(value)
                .build()
                .unwrap(),
        );
        if let Some(p) = prefix {
            and = and.prefix(p);
        }
        if extra_tag {
            and = and.tags(Tag::builder().key("team").value("ops").build().unwrap());
        }
        if let Some(n) = size_floor {
            and = and.object_size_greater_than(n);
        }
        LifecycleRule::builder()
            .id(format!("and-{value}"))
            .status(ExpirationStatus::Enabled)
            .filter(LifecycleRuleFilter::builder().and(and.build()).build())
            .expiration(LifecycleExpiration::builder().days(days).build())
            .build()
            .expect("rule")
    }

    #[test]
    fn a_prefix_scoped_rule_covering_the_silo_is_accepted() {
        // The obvious shape for a shared bucket. Rejecting it would report a
        // working configuration as broken across all five classes — the cry-wolf
        // failure this verifier is supposed to avoid, from the other side.
        for prefix in ["", "customer-app-storage/", "customer-app-"] {
            let rule = and_rule("30d", 30, Some(prefix), false, None);
            assert_eq!(
                classify_rule(&[rule], TtlClass::D30),
                ClassStatus::Satisfied,
                "prefix {prefix:?} covers the whole silo and must be accepted"
            );
        }
    }

    /// Assert a rule lands in [`ClassStatus::Narrowed`] and hand back its reason.
    fn narrowed_reason(rule: LifecycleRule) -> String {
        match classify_rule(&[rule], TtlClass::D30) {
            ClassStatus::Narrowed { reason, rule_id } => {
                // The id is what makes the log actionable — it is what an
                // operator greps for in the Terraform.
                assert!(rule_id.is_some(), "a narrowed rule must name itself");
                reason.to_string()
            }
            other => panic!("expected Narrowed, got {other:?}"),
        }
    }

    #[test]
    fn a_narrowed_rule_is_reported_as_narrowed_not_missing() {
        // Each of these leaves part of the class unexpiring. Reporting them as
        // `Missing` — "no rule expires this tag" — is false in both halves, and
        // an operator who greps the Terraform, finds the rule, and reads that it
        // does not exist stops believing the log.
        //
        // These three assertions are also the only thing standing between a
        // relaxed predicate and a green suite: flip `and.tags().len() > 1` to
        // `>= 1`, or delete the size-bound check, and without them a rule that
        // expires almost nothing reports Satisfied.
        assert!(
            narrowed_reason(and_rule(
                "30d",
                30,
                Some("customer-app-storage/uploads/"),
                false,
                None
            ))
            .contains("prefix")
        );
        assert!(narrowed_reason(and_rule("30d", 30, None, true, None)).contains("another tag"));
        assert!(
            narrowed_reason(and_rule("30d", 30, None, false, Some(1024))).contains("object size")
        );
    }

    #[test]
    fn a_disjoint_prefix_is_reported_without_overstating_what_survives() {
        // `logs/` shares nothing with the silo, so the rule selects NO object of
        // the class — "only part of it expires" would be generous by exactly one
        // part. The wording covers both the deeper and the disjoint case.
        let reason = narrowed_reason(and_rule("30d", 30, Some("logs/"), false, None));
        assert!(reason.contains("does not cover"), "{reason}");
    }

    #[test]
    fn a_narrowed_report_names_the_offending_rule() {
        // The id has to survive into the operator-facing string, or the log says
        // something is wrong without saying which line to edit.
        let report = VerifyReport {
            classes: vec![(
                TtlClass::D30,
                ClassStatus::Narrowed {
                    rule_id: Some("expire-oxy-ttl-30d".to_string()),
                    reason: "is bounded by object size",
                },
            )],
        };
        assert!(!report.is_satisfied());
        let problems = report.problems();
        assert_eq!(problems.len(), 1);
        assert!(
            problems[0].contains("expire-oxy-ttl-30d"),
            "{}",
            problems[0]
        );
        assert!(problems[0].contains("object size"), "{}", problems[0]);
    }

    #[test]
    fn an_unnamed_narrowed_rule_still_produces_a_message() {
        // `id` is optional on a LifecycleRule, and the fallback is the only path
        // that would panic on unwrap if it were written that way.
        let report = VerifyReport {
            classes: vec![(
                TtlClass::D30,
                ClassStatus::Narrowed {
                    rule_id: None,
                    reason: "also requires another tag",
                },
            )],
        };
        assert!(report.problems()[0].contains("<unnamed>"));
    }

    #[test]
    fn a_covering_rule_with_the_wrong_days_outranks_a_narrowed_one() {
        // A bucket can hold both at once, and the two diagnostics send an
        // operator to different places. `Mismatched` names the rule that already
        // selects the whole class and only has the wrong day count — fixing that
        // one number fixes the class outright, whereas chasing the narrowed rule
        // leaves the real rule still expiring at the wrong age.
        //
        // Nothing else pins this: swap the two blocks in `classify_rule` and
        // every other test still passes.
        let wrong_days = ttl_rule("30d", 60, true);
        let narrow = and_rule(
            "30d",
            30,
            Some("customer-app-storage/uploads/"),
            false,
            None,
        );
        for rules in [
            vec![wrong_days.clone(), narrow.clone()],
            vec![narrow, wrong_days],
        ] {
            assert_eq!(
                classify_rule(&rules, TtlClass::D30),
                ClassStatus::Mismatched {
                    found_days: Some(60),
                    enabled: true
                },
                "a covering rule's wrong day count outranks a narrowed rule, either order"
            );
        }
    }

    #[test]
    fn a_covering_rule_beats_a_narrowed_one_whatever_the_order() {
        // A bucket carrying both a scoped legacy rule and a correct one is
        // satisfied; the narrowed one is only a witness when nothing covers.
        let narrow = and_rule(
            "30d",
            30,
            Some("customer-app-storage/uploads/"),
            false,
            None,
        );
        let good = ttl_rule("30d", 30, true);
        assert_eq!(
            classify_rule(&[narrow.clone(), good.clone()], TtlClass::D30),
            ClassStatus::Satisfied
        );
        assert_eq!(
            classify_rule(&[good, narrow], TtlClass::D30),
            ClassStatus::Satisfied
        );
    }

    #[test]
    fn a_live_rule_wins_over_a_stale_one_ordered_ahead_of_it() {
        // The state a bucket passes through during a Terraform migration that
        // adds the replacement before removing the original. Taking the first
        // match would call this a mismatch while retention is actually fine.
        let stale = ttl_rule("30d", 30, false);
        let live = ttl_rule("30d", 30, true);
        assert_eq!(
            classify_rule(&[stale, live], TtlClass::D30),
            ClassStatus::Satisfied
        );
    }

    #[test]
    fn prefix_covers_silo_tracks_app_prefix() {
        // SILO_ROOT is duplicated from `app_prefix`; if that scheme ever changes,
        // every prefix-scoped rule would start reading as Missing.
        let key = crate::storage::app_prefix(uuid::Uuid::from_u128(1));
        assert!(
            key.starts_with(SILO_ROOT),
            "app_prefix() produced {key:?}, which is not under SILO_ROOT {SILO_ROOT:?}"
        );
        assert!(!prefix_covers_silo("customer-app-storage/uploads/"));
    }

    #[test]
    fn a_report_names_every_unsatisfied_class() {
        let report = VerifyReport {
            classes: vec![
                (TtlClass::D1, ClassStatus::Satisfied),
                (TtlClass::D30, ClassStatus::Missing),
                (
                    TtlClass::D90,
                    ClassStatus::Mismatched {
                        found_days: Some(60),
                        enabled: true,
                    },
                ),
            ],
        };
        assert!(!report.is_satisfied());
        let problems = report.problems();
        assert_eq!(problems.len(), 2, "satisfied classes must not be reported");
        assert!(
            problems.iter().any(|p| p.contains("oxy-ttl=30d")),
            "{problems:?}"
        );
        assert!(problems.iter().any(|p| p.contains("60")), "{problems:?}");
    }

    #[test]
    fn a_fully_configured_bucket_reports_nothing() {
        let report = VerifyReport {
            classes: TtlClass::ALL
                .iter()
                .map(|&c| (c, ClassStatus::Satisfied))
                .collect(),
        };
        assert!(report.is_satisfied());
        assert!(report.problems().is_empty());
    }

    #[test]
    fn no_such_lifecycle_configuration_is_recognized() {
        assert!(is_no_such_lifecycle_configuration(
            "service error: NoSuchLifecycleConfiguration: The lifecycle configuration does not exist"
        ));
        assert!(!is_no_such_lifecycle_configuration("AccessDenied"));
    }
}
