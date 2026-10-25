//! The release timeline's Web Vitals, per production release and screen.
//!
//! Only rows that name their build (`payload.build`, sent by `runtime.js`
//! since vitals carried it) are read, and a row counts only inside its own
//! build's release: there is no guessing a page's build from its session.
//!
//! **Postgres aggregates in SQL** and returns at most `MAX_SCREENS + 1` rows
//! per release. **Airhouse cannot**: nothing promises DuckDB's JSON functions
//! are loaded there. So its read keeps to the rows that name a compared build
//! (a `LIKE` on the payload, which `serde_json` writes without spaces), takes
//! at most [`AIRHOUSE_SAMPLES_PER_RELEASE`] of each release's newest page
//! loads, and [`aggregate`] computes the same p75 (`percentile_cont`, rounded
//! half to even) and the same ranking (samples, then path in byte order) as
//! the SQL. A test holds the two to the same answer.
//!
//! The payload is written by the page's own JavaScript, so every value is
//! bounded **before** the percentile: a non-number, a negative, an LCP over a
//! minute or a `1e400` is not a sample. A bound applied after the aggregate
//! could not take one forged value back out of a p75. On Postgres the bound is
//! applied in SQL, where a cast of `1e400` to `float8` would otherwise fail the
//! whole statement.
//!
//! **Time.** The Airhouse attempt has its own deadline, shorter than the
//! caller's, and running out of it falls back to Postgres and starts the
//! workspace's read backoff like any other failure.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, FromQueryResult, Statement,
    TransactionTrait,
};
use serde_json::Value;
use uuid::Uuid;

use super::{airhouse_workspace_of, facts, or_fall_back, query};
use crate::usage_airhouse::{SinkError, micros, ts_literal};
use oxy_cameras::airhouse::escape::sql_str;

/// Screens shown per release, most page loads first.
pub const MAX_SCREENS: usize = 8;

/// The largest sample each vital keeps, in the unit the runtime sends: LCP and
/// INP in milliseconds, CLS × 1000.
const LCP_MAX: f64 = 60_000.0;
const INP_MAX: f64 = 30_000.0;
const CLS_MAX: f64 = 10_000.0;
/// A screen's path is cut to this many characters.
const PATH_CHARS: usize = 256;
/// The most page loads an Airhouse read takes per release, newest first. A
/// release with more is measured over its newest ones.
pub const AIRHOUSE_SAMPLES_PER_RELEASE: usize = 5_000;

/// One production release whose vitals are read: `[starts_at, ends_at)`, and
/// only rows naming `build_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VitalsStretch {
    pub idx: i64,
    pub build_id: Uuid,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
}

/// One screen of one release. `cls_p75` is CLS × 1000, as the runtime sends it.
#[derive(Debug, Clone, PartialEq, Eq, FromQueryResult)]
pub struct VitalsRow {
    pub idx: i64,
    pub path: String,
    pub samples: i64,
    pub lcp_p75: Option<i64>,
    pub inp_p75: Option<i64>,
    pub cls_p75: Option<i64>,
    pub rank: i64,
}

/// One `oxy-web-vitals` event whose path is a string, its values already
/// bounded (`None`: absent or not a sample).
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub at: DateTime<Utc>,
    pub path: String,
    pub build: Option<String>,
    pub lcp: Option<f64>,
    pub inp: Option<f64>,
    pub cls: Option<f64>,
}

/// Every release's screens, up to [`MAX_SCREENS`] each, plus one more where
/// there are more (`rank > MAX_SCREENS`), which the caller reports as a count.
/// An Airhouse read gets `airhouse_timeout`; a Postgres read is cut off after
/// `statement_timeout_ms`.
pub async fn release_vitals(
    db: &DatabaseConnection,
    app_id: Uuid,
    stretches: &[VitalsStretch],
    airhouse_timeout: Duration,
    statement_timeout_ms: u64,
) -> Result<Vec<VitalsRow>, DbErr> {
    let stretches: Vec<VitalsStretch> = stretches
        .iter()
        .map(|s| VitalsStretch {
            starts_at: micros(s.starts_at),
            ends_at: micros(s.ends_at),
            ..*s
        })
        .collect();
    let Some((lo, hi)) = bounds(&stretches) else {
        return Ok(Vec::new());
    };
    if let Some(workspace_id) = airhouse_workspace_of(db, app_id).await? {
        let read = airhouse_samples(db, workspace_id, app_id, &stretches, lo, hi);
        let result = tokio::time::timeout(airhouse_timeout, read)
            .await
            .unwrap_or_else(|_| {
                Err(SinkError::Failed(format!(
                    "the vitals read timed out after {}ms",
                    airhouse_timeout.as_millis()
                )))
            });
        if let Some(samples) = or_fall_back(workspace_id, "the release vitals", result) {
            return Ok(aggregate(&stretches, &samples));
        }
    }
    postgres_vitals(db, app_id, &stretches, statement_timeout_ms).await
}

fn bounds(stretches: &[VitalsStretch]) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let lo = stretches.iter().map(|s| s.starts_at).min()?;
    let hi = stretches.iter().map(|s| s.ends_at).max()?;
    Some((lo, hi))
}

async fn airhouse_samples(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    app_id: Uuid,
    stretches: &[VitalsStretch],
    lo: DateTime<Utc>,
    hi: DateTime<Utc>,
) -> Result<Vec<Sample>, SinkError> {
    // Each release with the payload text its build leaves: `serde_json` writes
    // `"build":"<id>"` with no space, and a uuid holds no `LIKE` wildcard.
    let releases = stretches
        .iter()
        .map(|s| {
            format!(
                "({}, {}, {}, {})",
                s.idx,
                sql_str(&format!("%\"build\":\"{}\"%", s.build_id)),
                ts_literal(s.starts_at),
                ts_literal(s.ends_at),
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "WITH u AS ({facts}), \
         s AS (SELECT * FROM (VALUES {releases}) AS s(idx, pattern, starts_at, ends_at)) \
         SELECT s.idx::BIGINT AS idx, epoch_us(u.occurred_at) AS at_us, u.payload \
         FROM u JOIN s ON u.occurred_at >= s.starts_at AND u.occurred_at < s.ends_at \
                      AND u.payload LIKE s.pattern \
         WHERE u.kind = 'event' AND u.event_name = 'oxy-web-vitals' \
         QUALIFY row_number() OVER (PARTITION BY s.idx ORDER BY u.occurred_at DESC) \
                 <= {AIRHOUSE_SAMPLES_PER_RELEASE}",
        facts = facts(&[app_id], Some(lo), Some(hi)),
    );
    let rows = query(db, workspace_id, &sql).await?;
    let mut per_release: HashMap<i64, usize> = HashMap::new();
    let mut out = Vec::new();
    for row in rows.iter() {
        *per_release.entry(row.int("idx")?).or_default() += 1;
        let at = row.ts("at_us")?;
        let payload = row.opt_text("payload")?.unwrap_or_default();
        if let Some(sample) = sample_of(at, &payload) {
            out.push(sample);
        }
    }
    let capped = per_release
        .values()
        .filter(|n| **n >= AIRHOUSE_SAMPLES_PER_RELEASE)
        .count();
    if capped > 0 {
        tracing::info!(
            %app_id,
            releases = capped,
            "custom-app vitals: a release had more page loads than one read takes; \
             measured over its newest {AIRHOUSE_SAMPLES_PER_RELEASE}"
        );
    }
    Ok(out)
}

/// The sample a stored payload is worth: `None` when the path is not a
/// string, exactly the rows the Postgres read leaves out.
pub fn sample_of(at: DateTime<Utc>, payload: &str) -> Option<Sample> {
    let payload: Value = serde_json::from_str(payload).ok()?;
    let path = payload.get("path")?.as_str()?;
    let bounded = |key: &str, max: f64| {
        let value = payload.get(key)?;
        let n = value.as_f64().filter(|_| value.is_number())?;
        (0.0..=max).contains(&n).then_some(n)
    };
    Some(Sample {
        at,
        path: path.chars().take(PATH_CHARS).collect(),
        build: payload
            .get("build")
            .and_then(Value::as_str)
            .map(str::to_string),
        lcp: bounded("lcp", LCP_MAX),
        inp: bounded("inp", INP_MAX),
        cls: bounded("cls", CLS_MAX),
    })
}

/// The p75 of payload key `key`, its samples bounded to `0..=max`. Nested
/// `CASE`s, because SQL does not promise the order an `AND` is evaluated in,
/// and the cast must only ever see a jsonb number. `round` of a `float8`
/// rounds half to even, as [`p75`] does.
fn p75_sql(key: &str, max: f64) -> String {
    format!(
        "round(percentile_cont(0.75) WITHIN GROUP (ORDER BY \
           CASE WHEN jsonb_typeof(e.payload->'{key}') = 'number' THEN \
             CASE WHEN (e.payload->>'{key}')::numeric BETWEEN 0 AND {max} \
               THEN (e.payload->>'{key}')::numeric::float8 END END))::bigint AS {key}_p75"
    )
}

/// `$1` the app; `$2`–`$5` the releases' indexes, builds, starts and ends.
fn postgres_sql() -> String {
    format!(
        "WITH s AS (SELECT * FROM unnest($2::bigint[], $3::text[], $4::timestamptz[], \
                    $5::timestamptz[]) AS s(idx, build, starts_at, ends_at)), \
         g AS ( \
           SELECT s.idx, left(e.payload->>'path', {PATH_CHARS}) AS path, \
                  COUNT(*)::bigint AS samples, {lcp}, {inp}, {cls} \
           FROM s JOIN custom_app_event e \
             ON e.app_id = $1 AND e.event_name = 'oxy-web-vitals' \
            AND e.environment = 'production' \
            AND e.occurred_at >= s.starts_at AND e.occurred_at < s.ends_at \
            AND e.payload->>'build' = s.build \
           WHERE jsonb_typeof(e.payload->'path') = 'string' \
           GROUP BY 1, 2), \
         r AS (SELECT g.*, row_number() OVER (PARTITION BY idx \
                 ORDER BY samples DESC, path COLLATE \"C\") AS rank FROM g) \
         SELECT idx, path, samples, lcp_p75, inp_p75, cls_p75, rank FROM r \
         WHERE rank <= {limit} ORDER BY idx, rank",
        lcp = p75_sql("lcp", LCP_MAX),
        inp = p75_sql("inp", INP_MAX),
        cls = p75_sql("cls", CLS_MAX),
        limit = MAX_SCREENS + 1,
    )
}

async fn postgres_vitals(
    db: &DatabaseConnection,
    app_id: Uuid,
    stretches: &[VitalsStretch],
    statement_timeout_ms: u64,
) -> Result<Vec<VitalsRow>, DbErr> {
    let idx: Vec<i64> = stretches.iter().map(|s| s.idx).collect();
    let builds: Vec<String> = stretches.iter().map(|s| s.build_id.to_string()).collect();
    let starts: Vec<DateTime<Utc>> = stretches.iter().map(|s| s.starts_at).collect();
    let ends: Vec<DateTime<Utc>> = stretches.iter().map(|s| s.ends_at).collect();
    let txn = db.begin().await?;
    txn.execute_unprepared(&format!(
        "SET LOCAL statement_timeout = {statement_timeout_ms}"
    ))
    .await?;
    let rows = VitalsRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        postgres_sql(),
        [
            app_id.into(),
            idx.into(),
            builds.into(),
            starts.into(),
            ends.into(),
        ],
    ))
    .all(&txn)
    .await?;
    txn.commit().await?;
    Ok(rows)
}

/// Each stretch's screens: a sample counts in a stretch when it falls inside
/// it and names the stretch's build. Ranked by samples, most first, then by
/// path in byte order; ranks past `MAX_SCREENS + 1` are dropped.
pub fn aggregate(stretches: &[VitalsStretch], samples: &[Sample]) -> Vec<VitalsRow> {
    let mut out = Vec::new();
    let mut stretches = stretches.to_vec();
    stretches.sort_by_key(|s| s.idx);
    for stretch in &stretches {
        let build = stretch.build_id.to_string();
        let mut screens: HashMap<&str, Vec<&Sample>> = HashMap::new();
        for sample in samples {
            if sample.at >= stretch.starts_at
                && sample.at < stretch.ends_at
                && sample.build.as_deref() == Some(build.as_str())
            {
                screens
                    .entry(sample.path.as_str())
                    .or_default()
                    .push(sample);
            }
        }
        let mut rows: Vec<VitalsRow> = screens
            .into_iter()
            .map(|(path, of)| VitalsRow {
                idx: stretch.idx,
                path: path.to_string(),
                samples: of.len() as i64,
                lcp_p75: p75(of.iter().filter_map(|s| s.lcp)),
                inp_p75: p75(of.iter().filter_map(|s| s.inp)),
                cls_p75: p75(of.iter().filter_map(|s| s.cls)),
                rank: 0,
            })
            .collect();
        rows.sort_by(|a, b| b.samples.cmp(&a.samples).then_with(|| a.path.cmp(&b.path)));
        for (i, row) in rows.iter_mut().enumerate() {
            row.rank = i as i64 + 1;
        }
        rows.truncate(MAX_SCREENS + 1);
        out.extend(rows);
    }
    out
}

/// The 75th percentile with linear interpolation between the two nearest
/// samples (Postgres' `percentile_cont(0.75)`), rounded half to even. `None`
/// for no samples.
pub fn p75(values: impl Iterator<Item = f64>) -> Option<i64> {
    let mut sorted: Vec<f64> = values.collect();
    if sorted.is_empty() {
        return None;
    }
    sorted.sort_by(f64::total_cmp);
    let pos = 0.75 * (sorted.len() - 1) as f64;
    let (lo, hi) = (pos.floor() as usize, pos.ceil() as usize);
    let value = sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo as f64);
    Some(value.round_ties_even() as i64)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use chrono::TimeZone;

    /// Screens per stretch, keyed for tests: `idx → [(path, samples, rank)]`.
    fn shape(rows: &[VitalsRow]) -> BTreeMap<i64, Vec<(String, i64, i64)>> {
        let mut out: BTreeMap<i64, Vec<(String, i64, i64)>> = BTreeMap::new();
        for r in rows {
            out.entry(r.idx)
                .or_default()
                .push((r.path.clone(), r.samples, r.rank));
        }
        out
    }

    fn t(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_790_000_000 + secs, 0).unwrap()
    }

    #[test]
    fn p75_interpolates_like_percentile_cont_and_rounds_half_to_even() {
        assert_eq!(p75([].into_iter()), None);
        assert_eq!(p75([7.0].into_iter()), Some(7));
        // pos = 0.75 * 4 = 3: the fourth value, sorted.
        assert_eq!(p75([10.0, 50.0, 40.0, 20.0, 30.0].into_iter()), Some(40));
        // pos = 0.75 * 3 = 2.25: 30 + 0.25 * (44 - 30) = 33.5, to the even 34.
        assert_eq!(p75([10.0, 44.0, 20.0, 30.0].into_iter()), Some(34));
        // pos = 0.75: 1 + 0.75 * (3 - 1) = 2.5, a tie, to the even 2.
        assert_eq!(p75([1.0, 3.0].into_iter()), Some(2));
    }

    #[test]
    fn a_payload_value_outside_its_bound_or_not_a_number_is_not_a_sample() {
        let s = sample_of(
            t(0),
            r#"{"path":"/a","build":"b","lcp":60001,"inp":"12","cls":-1}"#,
        )
        .unwrap();
        assert_eq!((s.lcp, s.inp, s.cls), (None, None, None));
        let s = sample_of(t(0), r#"{"path":"/a","lcp":60000,"inp":0,"cls":2.5}"#).unwrap();
        assert_eq!((s.lcp, s.inp, s.cls), (Some(60000.0), Some(0.0), Some(2.5)));
        assert_eq!(
            sample_of(t(0), r#"{"path":7}"#),
            None,
            "path is not a string"
        );
        assert_eq!(sample_of(t(0), "not json"), None);
        let long = format!(r#"{{"path":"{}"}}"#, "x".repeat(300));
        assert_eq!(sample_of(t(0), &long).unwrap().path.len(), PATH_CHARS);
    }

    #[test]
    fn a_sample_counts_only_inside_its_own_builds_stretch() {
        let (a, b) = (Uuid::from_u128(1), Uuid::from_u128(2));
        let stretches = [
            VitalsStretch {
                idx: 0,
                build_id: a,
                starts_at: t(0),
                ends_at: t(100),
            },
            VitalsStretch {
                idx: 1,
                build_id: b,
                starts_at: t(100),
                ends_at: t(200),
            },
        ];
        let sample = |at, build: Uuid, path: &str| Sample {
            at: t(at),
            path: path.into(),
            build: Some(build.to_string()),
            lcp: Some(1000.0),
            inp: None,
            cls: None,
        };
        let samples = [
            sample(10, a, "/home"),
            sample(20, a, "/home"),
            sample(30, a, "/orders"),
            sample(150, a, "/home"), // a's build, b's stretch: no stretch
            sample(150, b, "/orders"),
            sample(200, b, "/orders"), // the end is exclusive
        ];
        let rows = aggregate(&stretches, &samples);
        assert_eq!(
            shape(&rows),
            BTreeMap::from([
                (0, vec![("/home".into(), 2, 1), ("/orders".into(), 1, 2)]),
                (1, vec![("/orders".into(), 1, 1)]),
            ])
        );
        assert_eq!(rows[0].lcp_p75, Some(1000));
        assert_eq!(rows[0].inp_p75, None);
    }

    #[test]
    fn one_screen_past_the_limit_is_kept_to_say_there_are_more() {
        let a = Uuid::from_u128(1);
        let stretches = [VitalsStretch {
            idx: 0,
            build_id: a,
            starts_at: t(0),
            ends_at: t(100),
        }];
        let samples: Vec<Sample> = (0..12)
            .map(|i| Sample {
                at: t(i),
                path: format!("/p{i:02}"),
                build: Some(a.to_string()),
                lcp: None,
                inp: None,
                cls: None,
            })
            .collect();
        let rows = aggregate(&stretches, &samples);
        assert_eq!(rows.len(), MAX_SCREENS + 1);
        assert_eq!(
            rows.last().unwrap().path,
            "/p08",
            "ties go to the path, in order"
        );
    }
}
