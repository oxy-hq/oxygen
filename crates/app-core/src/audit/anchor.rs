//! Anchor the audit chain heads **outside the database**, under S3 Object Lock.
//!
//! The hash chain in `audit_events` proves a row was not edited *given the
//! chain head is trusted* — and anyone who can rewrite rows can rewrite the
//! whole chain and its head. The fix that costs one bucket, not a new vendor:
//! every hour, write each org's current `(seq, hash)` to an object that S3
//! itself refuses to delete or overwrite until its retention expires
//! (Object Lock, **compliance** mode — undeletable even by the bucket owner).
//! A verifier then checks the database against the anchor, not against itself.
//!
//! What lands in the bucket is a digest of identifiers — never an audit row.
//!
//! Configuration (all env; unset bucket = the job does nothing, loudly once):
//!
//! | Variable | Default | Meaning |
//! | --- | --- | --- |
//! | `OXY_AUDIT_ANCHOR_S3_BUCKET` | — | The bucket, created **with Object Lock enabled** (Terraform: `object_lock_enabled = true`, which forces versioning). A bucket without it fails every put. |
//! | `OXY_AUDIT_ANCHOR_KEY_PREFIX` | `audit-anchors` | Key prefix. |
//! | `OXY_AUDIT_ANCHOR_INTERVAL_SECS` | `3600` | Cadence (min 60). |
//! | `OXY_AUDIT_ANCHOR_RETENTION_DAYS` | `400` | Lock duration per object (min 1). Compliance-mode retention cannot be shortened after the fact. |
//!
//! Keys are `<prefix>/<YYYY>/<MM>/<DD>/<HHMM>.json`, so lexical order is time
//! order, plus an unlocked `<prefix>/latest.json` pointer that names the most
//! recent locked key. The pointer is a convenience: a verifier that distrusts
//! it lists the day's prefix instead.
//!
//! Down from `oxy-app`'s `server::audit_anchor` in the admin split's PR 3, so
//! the console's verifier (`GET /admin/audit/anchor/{org_id}`) can leave
//! `oxy-app` with `admin::audit` (it did, for `oxy-api-admin`, on
//! 2026-11-20). The hourly loop that writes anchors stayed in `oxy-app`
//! (`server::audit_anchor`), and is `oxy-server`'s `drivers::audit_anchor`
//! since decompose BL1 (2026-11-23): it builds the custom-app store's S3
//! client, which lives above this crate.

use std::time::Duration;

use aws_sdk_s3::Client as S3Client;
use aws_sdk_s3::primitives::{ByteStream, DateTime as AwsDateTime};
use aws_sdk_s3::types::ObjectLockMode;
use chrono::{DateTime, Utc};
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::audit::{self, ChainHead};

pub const BUCKET_ENV: &str = "OXY_AUDIT_ANCHOR_S3_BUCKET";
pub const PREFIX_ENV: &str = "OXY_AUDIT_ANCHOR_KEY_PREFIX";
pub const INTERVAL_ENV: &str = "OXY_AUDIT_ANCHOR_INTERVAL_SECS";
pub const RETENTION_ENV: &str = "OXY_AUDIT_ANCHOR_RETENTION_DAYS";

const DEFAULT_PREFIX: &str = "audit-anchors";
const DEFAULT_INTERVAL_SECS: u64 = 3600;
const MIN_INTERVAL_SECS: u64 = 60;
const DEFAULT_RETENTION_DAYS: i64 = 400;

/// The format version inside every anchor document.
pub const DOCUMENT_VERSION: u32 = 1;

/// An env value parsed with a warning on garbage, so a typo does not
/// silently become the default for an hour.
fn parsed_or<T: std::str::FromStr + Copy>(value: Option<String>, var: &str, default: T) -> T {
    match value {
        None => default,
        Some(v) => v.trim().parse().unwrap_or_else(|_| {
            tracing::warn!(var, value = %v, "unparseable value; using the default");
            default
        }),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnchorConfig {
    pub bucket: String,
    pub prefix: String,
    pub interval: Duration,
    pub retention_days: i64,
}

impl AnchorConfig {
    /// `None` when no bucket is configured — the feature is off.
    pub fn from_env() -> Option<Self> {
        Self::from_values(
            std::env::var(BUCKET_ENV).ok(),
            std::env::var(PREFIX_ENV).ok(),
            std::env::var(INTERVAL_ENV).ok(),
            std::env::var(RETENTION_ENV).ok(),
        )
    }

    fn from_values(
        bucket: Option<String>,
        prefix: Option<String>,
        interval: Option<String>,
        retention: Option<String>,
    ) -> Option<Self> {
        let bucket = bucket
            .map(|b| b.trim().to_string())
            .filter(|b| !b.is_empty())?;
        let prefix = prefix
            .map(|p| p.trim().trim_matches('/').to_string())
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| DEFAULT_PREFIX.to_string());
        let secs = parsed_or(interval, INTERVAL_ENV, DEFAULT_INTERVAL_SECS).max(MIN_INTERVAL_SECS);
        let retention_days = parsed_or(retention, RETENTION_ENV, DEFAULT_RETENTION_DAYS).max(1);
        Some(Self {
            bucket,
            prefix,
            interval: Duration::from_secs(secs),
            retention_days,
        })
    }

    /// The most recent locked key, from the pointer and the listing together:
    /// the pointer is unlocked and therefore rewritable, so it is never the
    /// authority — the higher key wins, and a pointer that names a key the
    /// listing does not exceed keeps its digest for [`fetch_document`] to
    /// check. Pure, so the choice is testable without S3.
    pub fn pick_latest(
        pointer: Option<&AnchorPointer>,
        listed: impl IntoIterator<Item = String>,
    ) -> Option<(String, Option<String>)> {
        let listed_max = listed.into_iter().max();
        match (pointer, listed_max) {
            // The pointer names the newest listed object: use its digest.
            (Some(p), Some(l)) if l == p.key => Some((l, Some(p.sha256.clone()))),
            // A pointer rolled forward names an object that is not there —
            // the listing is what exists. A pointer rolled back is simply
            // outranked.
            (_, Some(l)) => Some((l, None)),
            // Nothing listed in the last two days: an old pointer is still
            // worth a look (the job may have been off), digest-checked.
            (Some(p), None) => Some((p.key.clone(), Some(p.sha256.clone()))),
            (None, None) => None,
        }
    }

    /// The locked object for an anchor taken at `at`.
    pub fn object_key(&self, at: DateTime<Utc>) -> String {
        format!("{}/{}.json", self.prefix, at.format("%Y/%m/%d/%H%M"))
    }

    /// The unlocked pointer to the latest locked object.
    pub fn pointer_key(&self) -> String {
        format!("{}/latest.json", self.prefix)
    }

    /// The day prefix a verifier lists when it distrusts the pointer.
    pub fn day_prefix(&self, at: DateTime<Utc>) -> String {
        format!("{}/{}/", self.prefix, at.format("%Y/%m/%d"))
    }

    pub fn retain_until(&self, at: DateTime<Utc>) -> DateTime<Utc> {
        at + chrono::Duration::days(self.retention_days)
    }
}

/// What one anchor object holds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AnchorDocument {
    pub version: u32,
    pub anchored_at: DateTime<Utc>,
    pub heads: Vec<AnchoredHead>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AnchoredHead {
    pub org_id: Uuid,
    pub seq: i64,
    pub hash: String,
    pub created_at: DateTime<Utc>,
}

impl From<ChainHead> for AnchoredHead {
    fn from(h: ChainHead) -> Self {
        Self {
            org_id: h.org_id,
            seq: h.seq,
            hash: h.hash,
            created_at: h.created_at.with_timezone(&Utc),
        }
    }
}

/// The unlocked pointer's shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AnchorPointer {
    pub key: String,
    pub anchored_at: DateTime<Utc>,
    pub sha256: String,
}

/// Why a run did not anchor.
#[derive(Debug)]
pub enum AnchorError {
    /// The minute's object already exists — a second start inside the same
    /// wall-clock minute, not a misconfigured bucket.
    AlreadyAnchored(String),
    Other(String),
}

impl std::fmt::Display for AnchorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyAnchored(key) => write!(f, "{key} already exists for this minute"),
            Self::Other(e) => f.write_str(e),
        }
    }
}

/// What one run wrote.
#[derive(Debug, Clone, Serialize)]
pub struct AnchorOutcome {
    pub key: String,
    pub heads: usize,
    pub sha256: String,
    pub retain_until: DateTime<Utc>,
}

/// The verifier's answer for one org.
#[derive(Debug, Clone, Serialize)]
pub struct AnchorReport {
    pub org_id: Uuid,
    /// `false` when no bucket is configured; every other field is then absent.
    pub configured: bool,
    pub anchor_key: Option<String>,
    pub anchored_at: Option<DateTime<Utc>>,
    /// How old the anchor is. A rolled-back pointer shows up here as an
    /// anchor far older than the hourly cadence.
    pub age_secs: Option<i64>,
    /// The latest anchor carried no head for this org (no events at that
    /// time). Distinct from `present == false`, which is a finding.
    pub head_absent: bool,
    pub anchored_seq: Option<i64>,
    pub anchored_hash: Option<String>,
    /// The anchored `seq` still exists for this org.
    pub present: bool,
    /// …and its stored hash equals the anchored one. `present && matches` is
    /// the statement "nothing up to the anchor was rewritten or removed".
    pub matches: bool,
    pub detail: Option<String>,
}

impl AnchorReport {
    fn empty(org_id: Uuid, configured: bool, detail: String) -> Self {
        Self {
            org_id,
            configured,
            anchor_key: None,
            anchored_at: None,
            age_secs: None,
            head_absent: false,
            anchored_seq: None,
            anchored_hash: None,
            present: false,
            matches: false,
            detail: Some(detail),
        }
    }

    fn unconfigured(org_id: Uuid) -> Self {
        Self::empty(
            org_id,
            false,
            format!("{BUCKET_ENV} is not set; anchoring is off"),
        )
    }

    fn no_anchor_yet(org_id: Uuid) -> Self {
        Self::empty(org_id, true, "no anchor object exists yet".into())
    }

    /// The verification could not be completed — an unlocked or lapsed
    /// object, a rewritten pointer, S3 or the database unreachable. Fail-closed
    /// (`present` and `matches` false) with the reason where the operator
    /// reads it, rather than a bare 500 with the reason in a log line.
    pub fn failed(org_id: Uuid, reason: String) -> Self {
        Self::empty(org_id, true, reason)
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

/// Take one anchor: read every chain head, write the locked object, then
/// move the pointer. Empty chains still anchor — "nothing yet" at a time is a
/// statement too.
pub async fn anchor_once(
    db: &DatabaseConnection,
    s3: &S3Client,
    cfg: &AnchorConfig,
) -> Result<AnchorOutcome, AnchorError> {
    let at = Utc::now();
    let heads = audit::chain_heads(db)
        .await
        .map_err(|e| AnchorError::Other(format!("reading chain heads: {e}")))?;
    let doc = AnchorDocument {
        version: DOCUMENT_VERSION,
        anchored_at: at,
        heads: heads.into_iter().map(AnchoredHead::from).collect(),
    };
    let body = serde_json::to_vec_pretty(&doc)
        .map_err(|e| AnchorError::Other(format!("serialising anchor: {e}")))?;
    let digest = sha256_hex(&body);
    let key = cfg.object_key(at);
    let retain_until = cfg.retain_until(at);
    s3.put_object()
        .bucket(&cfg.bucket)
        .key(&key)
        .content_type("application/json")
        .body(ByteStream::from(body))
        .object_lock_mode(ObjectLockMode::Compliance)
        .object_lock_retain_until_date(AwsDateTime::from_millis(retain_until.timestamp_millis()))
        // An anchor is written once; a second write to the same minute is a
        // bug, not a retry, and must not replace the locked object.
        .if_none_match("*")
        .send()
        .await
        .map_err(|e| {
            let text = format!("{}", aws_sdk_s3::error::DisplayErrorContext(&e));
            if text.contains("PreconditionFailed") || text.contains("412") {
                AnchorError::AlreadyAnchored(key.clone())
            } else {
                AnchorError::Other(format!("putting {key}: {text}"))
            }
        })?;
    let pointer = AnchorPointer {
        key: key.clone(),
        anchored_at: at,
        sha256: digest.clone(),
    };
    let pointer_body = serde_json::to_vec(&pointer)
        .map_err(|e| AnchorError::Other(format!("serialising pointer: {e}")))?;
    if let Err(e) = s3
        .put_object()
        .bucket(&cfg.bucket)
        .key(cfg.pointer_key())
        .content_type("application/json")
        .body(ByteStream::from(pointer_body))
        .send()
        .await
    {
        // The locked object is the record; a stale pointer only costs the
        // verifier a listing.
        tracing::warn!(error = %e, "audit anchor: locked object written, pointer not moved");
    }
    Ok(AnchorOutcome {
        key,
        heads: doc.heads.len(),
        sha256: digest,
        retain_until,
    })
}

/// Compare an org's chain with the most recent anchor.
pub async fn verify_latest(
    db: &DatabaseConnection,
    s3: &S3Client,
    cfg: Option<&AnchorConfig>,
    org_id: Uuid,
) -> Result<AnchorReport, String> {
    let Some(cfg) = cfg else {
        return Ok(AnchorReport::unconfigured(org_id));
    };
    let Some((key, expected_sha)) = latest_key(s3, cfg).await? else {
        return Ok(AnchorReport::no_anchor_yet(org_id));
    };
    let doc = fetch_document(s3, cfg, &key, expected_sha.as_deref()).await?;
    let age_secs = Some((Utc::now() - doc.anchored_at).num_seconds());
    let Some(head) = doc.heads.iter().find(|h| h.org_id == org_id) else {
        return Ok(AnchorReport {
            anchor_key: Some(key),
            anchored_at: Some(doc.anchored_at),
            age_secs,
            head_absent: true,
            ..AnchorReport::empty(
                org_id,
                true,
                "the latest anchor has no head for this org (no events at that time)".into(),
            )
        });
    };
    let stored = audit::hash_at(db, org_id, head.seq)
        .await
        .map_err(|e| format!("reading seq {} for {org_id}: {e}", head.seq))?;
    let present = stored.is_some();
    let matches = stored.as_ref().and_then(|h| h.as_deref()) == Some(head.hash.as_str());
    let detail = match &stored {
        Some(Some(_)) if matches => None,
        Some(Some(_)) => {
            Some("the row at the anchored seq no longer reproduces the anchored hash".into())
        }
        Some(None) => Some("the row at the anchored seq exists but carries no hash".into()),
        None => Some("the anchored seq is gone: a row was removed after it was anchored".into()),
    };
    Ok(AnchorReport {
        org_id,
        configured: true,
        anchor_key: Some(key),
        anchored_at: Some(doc.anchored_at),
        age_secs,
        head_absent: false,
        anchored_seq: Some(head.seq),
        anchored_hash: Some(head.hash.clone()),
        present,
        matches,
        detail,
    })
}

/// The most recent locked key and, when the pointer named it, the digest the
/// pointer claims. The listing always runs: the pointer is unlocked, so it is
/// an optimisation, never the authority (see [`AnchorConfig::pick_latest`]).
async fn latest_key(
    s3: &S3Client,
    cfg: &AnchorConfig,
) -> Result<Option<(String, Option<String>)>, String> {
    let pointer = match s3
        .get_object()
        .bucket(&cfg.bucket)
        .key(cfg.pointer_key())
        .send()
        .await
    {
        Ok(out) => out
            .body
            .collect()
            .await
            .ok()
            .and_then(|b| serde_json::from_slice::<AnchorPointer>(&b.into_bytes()).ok()),
        Err(_) => None,
    };
    let now = Utc::now();
    let mut listed = Vec::new();
    for day in [now, now - chrono::Duration::days(1)] {
        // Paginated: a day at the minimum cadence holds 1,440 objects, and S3
        // lists ascending, so a single page would stop short of the newest.
        let mut pages = s3
            .list_objects_v2()
            .bucket(&cfg.bucket)
            .prefix(cfg.day_prefix(day))
            .into_paginator()
            .send();
        while let Some(page) = pages.next().await {
            let page = page.map_err(|e| format!("listing anchors: {e}"))?;
            listed.extend(
                page.contents()
                    .iter()
                    .filter_map(|o| o.key().map(str::to_string)),
            );
        }
    }
    Ok(AnchorConfig::pick_latest(pointer.as_ref(), listed))
}

/// Fetch and parse one anchor object; when the key came from the pointer,
/// the body must reproduce the digest the pointer recorded.
async fn fetch_document(
    s3: &S3Client,
    cfg: &AnchorConfig,
    key: &str,
    expected_sha256: Option<&str>,
) -> Result<AnchorDocument, String> {
    let out = s3
        .get_object()
        .bucket(&cfg.bucket)
        .key(key)
        .send()
        .await
        .map_err(|e| format!("getting {key}: {e}"))?;
    // The retention is the whole guarantee: Object Lock stops an existing
    // object being changed, not a new key being created, so an object in the
    // prefix that is not itself locked is not an anchor, whoever put it
    // there — and is a finding in its own right, not a condition to route
    // around. (Reading it needs `s3:GetObjectRetention`.)
    let retain_until = out.object_lock_retain_until_date().map(|d| d.secs());
    match lock_state(out.object_lock_mode(), retain_until, Utc::now().timestamp()) {
        LockState::Locked => {}
        LockState::Lapsed => {
            let lapsed_at = retain_until
                .and_then(|secs| DateTime::<Utc>::from_timestamp(secs, 0))
                .map(|d| d.to_rfc3339())
                .unwrap_or_else(|| "unknown".into());
            return Err(format!(
                "{key}: the anchor's retention lapsed at {lapsed_at}; it is no longer \
                 immutable — the anchor job has been off longer than \
                 {RETENTION_ENV}, or the retention is too short"
            ));
        }
        LockState::Unlocked => {
            return Err(format!(
                "{key}: no Object Lock compliance retention visible (mode {:?}, retain until \
                 {retain_until:?}) — either the object is not locked (a finding in its own \
                 right) or the reading role lacks s3:GetObjectRetention, which makes S3 omit \
                 these headers entirely",
                out.object_lock_mode()
            ));
        }
    }
    let bytes = out
        .body
        .collect()
        .await
        .map_err(|e| format!("reading {key}: {e}"))?
        .into_bytes();
    if let Some(expected) = expected_sha256 {
        let actual = sha256_hex(&bytes);
        if actual != expected {
            return Err(format!(
                "{key}: the pointer's digest does not match the object it names \
                 (pointer {expected}, object {actual}); the pointer was rewritten"
            ));
        }
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("parsing {key}: {e}"))
}

/// What an object's lock metadata says about it as an anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockState {
    /// Compliance mode with retention still in the future: immutable, an anchor.
    Locked,
    /// Compliance mode whose retention has passed: it *was* an anchor and is
    /// no longer immutable — an operational condition (the job was down
    /// longer than the retention, or the retention is short), not an attack.
    Lapsed,
    /// No compliance lock visible: a planted object, a governance-mode lock
    /// (liftable), or a reader without `s3:GetObjectRetention`, for which S3
    /// omits the lock headers rather than failing.
    Unlocked,
}

/// Classify an object's lock metadata at `now` (unix seconds).
pub fn lock_state(
    mode: Option<&ObjectLockMode>,
    retain_until_secs: Option<i64>,
    now: i64,
) -> LockState {
    match (mode, retain_until_secs) {
        (Some(ObjectLockMode::Compliance), Some(until)) if until > now => LockState::Locked,
        (Some(ObjectLockMode::Compliance), Some(_)) => LockState::Lapsed,
        _ => LockState::Unlocked,
    }
}

/// The `fleet_job_claims` row the anchor loop claims each interval.
pub const ANCHOR_JOB: &str = "audit_chain_anchor";

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> AnchorConfig {
        AnchorConfig::from_values(
            Some("oxy-audit-anchors".into()),
            Some("/anchors/".into()),
            Some("30".into()),
            Some("0".into()),
        )
        .unwrap()
    }

    #[test]
    fn no_bucket_means_off_and_defaults_are_floored() {
        assert!(AnchorConfig::from_values(None, None, None, None).is_none());
        assert!(AnchorConfig::from_values(Some("  ".into()), None, None, None).is_none());
        let c = cfg();
        assert_eq!(c.prefix, "anchors", "slashes trimmed");
        assert_eq!(c.interval, Duration::from_secs(MIN_INTERVAL_SECS));
        assert_eq!(c.retention_days, 1);
        let d = AnchorConfig::from_values(Some("b".into()), None, None, None).unwrap();
        assert_eq!(d.prefix, DEFAULT_PREFIX);
        assert_eq!(d.interval, Duration::from_secs(DEFAULT_INTERVAL_SECS));
        assert_eq!(d.retention_days, DEFAULT_RETENTION_DAYS);
    }

    #[test]
    fn keys_sort_in_time_order_and_the_pointer_sits_beside_them() {
        let c = cfg();
        let t1 = DateTime::parse_from_rfc3339("2026-09-08T03:15:00Z")
            .unwrap()
            .to_utc();
        let t2 = DateTime::parse_from_rfc3339("2026-09-08T04:15:00Z")
            .unwrap()
            .to_utc();
        let t3 = DateTime::parse_from_rfc3339("2026-10-01T00:00:00Z")
            .unwrap()
            .to_utc();
        assert_eq!(c.object_key(t1), "anchors/2026/09/08/0315.json");
        assert!(c.object_key(t1) < c.object_key(t2));
        assert!(c.object_key(t2) < c.object_key(t3));
        assert_eq!(c.pointer_key(), "anchors/latest.json");
        assert_eq!(c.day_prefix(t1), "anchors/2026/09/08/");
        assert_eq!(c.retain_until(t1), t1 + chrono::Duration::days(1));
    }

    #[test]
    fn the_document_round_trips_and_digests_stably() {
        let doc = AnchorDocument {
            version: DOCUMENT_VERSION,
            anchored_at: DateTime::parse_from_rfc3339("2026-09-08T03:15:00Z")
                .unwrap()
                .to_utc(),
            heads: vec![AnchoredHead {
                org_id: Uuid::nil(),
                seq: 42,
                hash: "ab".repeat(32),
                created_at: DateTime::parse_from_rfc3339("2026-09-08T03:14:59Z")
                    .unwrap()
                    .to_utc(),
            }],
        };
        let body = serde_json::to_vec_pretty(&doc).unwrap();
        let back: AnchorDocument = serde_json::from_slice(&body).unwrap();
        assert_eq!(back, doc);
        assert_eq!(
            sha256_hex(&body),
            sha256_hex(&serde_json::to_vec_pretty(&doc).unwrap())
        );
        assert_eq!(sha256_hex(b"").len(), 64);
    }

    #[test]
    fn an_unconfigured_report_says_so_and_an_absent_head_is_not_a_finding() {
        let r = AnchorReport::unconfigured(Uuid::nil());
        assert!(!r.configured && !r.present && !r.matches && !r.head_absent);
        assert!(r.detail.unwrap().contains(BUCKET_ENV));
        let n = AnchorReport::no_anchor_yet(Uuid::nil());
        assert!(n.configured && n.anchor_key.is_none());
    }

    #[test]
    fn the_listing_outranks_a_rolled_back_pointer() {
        let p = AnchorPointer {
            key: "anchors/2026/09/08/0300.json".into(),
            anchored_at: Utc::now(),
            sha256: "abc".into(),
        };
        // Rolled back: the listing outranks it and no digest is trusted.
        assert_eq!(
            AnchorConfig::pick_latest(Some(&p), vec!["anchors/2026/09/08/0400.json".into()]),
            Some(("anchors/2026/09/08/0400.json".into(), None))
        );
        // Rolled forward: it names nothing that exists, so the listing wins.
        assert_eq!(
            AnchorConfig::pick_latest(Some(&p), vec!["anchors/2026/09/08/0200.json".into()]),
            Some(("anchors/2026/09/08/0200.json".into(), None))
        );
        // Current: it names the newest listed object, so its digest is used.
        assert_eq!(
            AnchorConfig::pick_latest(
                Some(&p),
                vec![p.key.clone(), "anchors/2026/09/08/0200.json".into()]
            ),
            Some((p.key.clone(), Some("abc".into())))
        );
        // Nothing listed for two days: the old pointer is still read, checked.
        assert_eq!(
            AnchorConfig::pick_latest(Some(&p), Vec::<String>::new()),
            Some((p.key.clone(), Some("abc".into())))
        );
        assert_eq!(
            AnchorConfig::pick_latest(None, vec!["a/1.json".into(), "a/2.json".into()]),
            Some(("a/2.json".into(), None))
        );
        assert_eq!(AnchorConfig::pick_latest(None, Vec::<String>::new()), None);
    }

    #[test]
    fn a_lapsed_anchor_is_told_apart_from_a_planted_object() {
        let now = 1_000;
        let c = Some(&ObjectLockMode::Compliance);
        assert_eq!(lock_state(c, Some(2_000), now), LockState::Locked);
        assert_eq!(
            lock_state(c, Some(999), now),
            LockState::Lapsed,
            "expired, not planted"
        );
        assert_eq!(lock_state(c, None, now), LockState::Unlocked);
        assert_eq!(
            lock_state(Some(&ObjectLockMode::Governance), Some(2_000), now),
            LockState::Unlocked,
            "governance can be lifted"
        );
        assert_eq!(
            lock_state(None, Some(2_000), now),
            LockState::Unlocked,
            "planted or unreadable"
        );
        let f = AnchorReport::failed(Uuid::nil(), "why".into());
        assert!(f.configured && !f.present && !f.matches);
        assert_eq!(f.detail.as_deref(), Some("why"));
    }

    #[test]
    fn garbage_env_values_fall_back_with_a_warning_not_silently_to_zero() {
        let c = AnchorConfig::from_values(
            Some("b".into()),
            None,
            Some("abc".into()),
            Some("many".into()),
        )
        .unwrap();
        assert_eq!(c.interval, Duration::from_secs(DEFAULT_INTERVAL_SECS));
        assert_eq!(c.retention_days, DEFAULT_RETENTION_DAYS);
    }
}
