//! The per-event bucket cap.
//!
//! A page bounds *events*; nothing bounds the buckets behind one, and
//! `metric_anomalies` has no retention — a long-running regime shift is a
//! single event with an unbounded chain. This trims within an event rather than
//! across a page, because `offset` counts events: a page-wide budget has to
//! drop whole events, and a dropped event is served by no page at all.
//!
//! It bounds the **payload**, not the query. Phase 2 fetches every bucket of
//! the ranked events and this drops rows afterwards, so a page of 25 events at
//! a thousand buckets each still materialises 25k `Model`s to return 1,250.
//! That is the shape the endpoint had before paging existed.
//!
//! The write path refuses an oversized selection outright (`MAX_BULK_ROWS`),
//! and the asymmetry is deliberate rather than an oversight: a write that is
//! too large has a correct answer — say no, the caller narrows it and repeats —
//! while a read has none. Refusing to list a workspace because one of its
//! events grew a long chain would make the inbox unusable exactly when someone
//! needs to look at it. Bounding the fetch instead of the payload needs a
//! per-event `ROW_NUMBER()` window in phase 2, so that the rows this function
//! would keep are the only ones read; that is the fix if it ever shows up in a
//! profile, and it has to preserve the reservations below — a plain `LIMIT`
//! would cut the peak as readily as the tail.
//!
//! What survives is what the row is built from. The client reads two different
//! things off a page — the severity badge is `MAX(severity)`, while period,
//! observed, expected and Δ% come from the global max-`|z|` bucket — and a row
//! outside the Dismissed tab offers no action at all if every bucket it
//! received is dismissed. So the peak and the best bucket of each status
//! present are reserved before anything is ranked.

use entity::metric_anomalies;
use oxy_metric_monitoring::detect::severity_rank;
use uuid::Uuid;

use super::list::event_key_of;

/// Keeps the buckets that *define* the row, on both of the axes the client
/// reads — which are genuinely two axes, not one.
///
/// The severity badge is `MAX(severity)` over what arrives, and severity comes
/// from headroom rather than from z, so a lone `high` breach can sit below
/// fifty `low` continuation buckets on `|z_score|` alone. The rest of the row —
/// period, observed, expected, Δ% — comes from the *global* max-`|z_score|`
/// bucket, whatever its severity.
///
/// So the survivors are: that peak bucket, unconditionally, plus the rest
/// ranked by severity then `|z_score|`. Ranking on severity alone would let the
/// row's numbers describe a bucket that isn't the worst one; ranking on z alone
/// would badge a real collapse `low` while `rank_event_keys` — which sorts the
/// page by `MAX(severity)` — still put it on top. Trimming by date loses both.
///
/// Assumes rows arrive grouped by event and ordered within it (the caller's
/// sort guarantees both, modulo unranked rows a concurrent scan may interleave
/// — vanishingly rare and self-correcting), and preserves that ordering.
pub(super) fn cap_buckets_per_event(rows: &mut Vec<metric_anomalies::Model>) -> Vec<String> {
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for row in rows.iter() {
        *counts.entry(event_key_of(row)).or_default() += 1;
    }
    if counts.values().all(|n| *n <= MAX_BUCKETS_PER_EVENT) {
        return Vec::new();
    }

    // Pick the survivors up front, by id. A "keep everything above the cap-th
    // worst |z|" threshold is not enough on its own: ties at the threshold let
    // an event exceed the cap, and resolving that with a running counter drops
    // whichever tied bucket comes last in date order — which can be the peak
    // itself, the one bucket that must never be trimmed.
    let mut by_event: std::collections::HashMap<String, Vec<Bucket>> =
        std::collections::HashMap::new();
    for row in rows.iter() {
        by_event.entry(event_key_of(row)).or_default().push(Bucket {
            severity: severity_rank(&row.severity),
            abs_z: row.z_score.abs(),
            id: row.id,
            status: row.status.clone(),
            period_start: row.period_start,
        });
    }
    // Internal keys while trimming; the client keys are derived at the end.
    let mut oversized: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut keep_ids: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
    for (key, mut buckets) in by_event {
        if buckets.len() <= MAX_BUCKETS_PER_EVENT {
            continue;
        }
        // Reserved before ranking, because ranking can drop either of them.
        //
        // The client's peak is the global max `|z|` ignoring severity, and the
        // row's numbers — period, observed, expected, Δ% — all come from it, so
        // a severity-first sort could trim the very bucket the row describes.
        //
        // And the best bucket of each status present, because the ranking is
        // status-blind: an All-tab event of 45 `dismissed` buckets and 20 mild
        // `new` ones can otherwise arrive looking wholly dismissed, and a
        // wholly-dismissed row offers no action outside the Dismissed tab — it
        // would render with nothing but Explain while the New badge went on
        // counting its `new` buckets. At most three rows, and they are what
        // tell the client which states this event is really in.
        let mut reserved: std::collections::HashSet<Uuid> = buckets
            .iter()
            .max_by(|a, b| {
                a.abs_z
                    .total_cmp(&b.abs_z)
                    // Earliest wins a tie, matching the client's first-maximum
                    // scan over period-ordered buckets; `id` only to stay
                    // deterministic when even that ties.
                    .then_with(|| b.period_start.cmp(&a.period_start))
                    .then_with(|| b.id.cmp(&a.id))
            })
            .map(|b| b.id)
            .into_iter()
            .collect();
        let mut best_of_status: std::collections::HashMap<&str, &Bucket> =
            std::collections::HashMap::new();
        for bucket in buckets.iter() {
            best_of_status
                .entry(bucket.status.as_str())
                .and_modify(|best| {
                    if bucket.rank() > best.rank() {
                        *best = bucket;
                    }
                })
                .or_insert(bucket);
        }
        reserved.extend(best_of_status.values().map(|b| b.id));

        // Worst first — severity, then |z| — with `id` breaking ties so the
        // choice is deterministic rather than dependent on row order.
        // `total_cmp` because a NaN `|z|` under `partial_cmp(..).unwrap_or(Equal)`
        // makes the comparator intransitive, which `sort_by` panics on.
        buckets.sort_by(|a, b| {
            b.severity
                .cmp(&a.severity)
                .then_with(|| b.abs_z.total_cmp(&a.abs_z))
                .then_with(|| a.id.cmp(&b.id))
        });
        // Per-event, not against `keep_ids` — that set spans every event on the
        // page, so counting it here would cap the page rather than the event.
        let mut kept = reserved;
        for bucket in &buckets {
            if kept.len() >= MAX_BUCKETS_PER_EVENT {
                break;
            }
            kept.insert(bucket.id);
        }
        keep_ids.extend(kept);
        oversized.insert(key);
    }

    let mut dropped = 0usize;
    rows.retain(|row| {
        if !oversized.contains(&event_key_of(row)) {
            return true;
        }
        let keep = keep_ids.contains(&row.id);
        if !keep {
            dropped += 1;
        }
        keep
    });
    // `debug`, not `warn`: this is a property of the data, not an incident, and
    // it recurs on every list request for as long as that event exists — each
    // scan poll, each Ack, each page turn, plus the badge's own query.
    tracing::debug!(
        dropped,
        cap = MAX_BUCKETS_PER_EVENT,
        "list_anomalies: an event exceeded the per-event bucket cap; its mildest buckets were trimmed"
    );
    // Translate to the client's key space on the way out — a bare id means
    // nothing to `groupIntoEvents`, which keys pre-event rows `ungrouped:<id>`.
    let mut truncated: Vec<String> = rows
        .iter()
        .filter(|row| oversized.contains(&event_key_of(row)))
        .map(client_event_key)
        .collect();
    truncated.sort();
    truncated.dedup();
    truncated
}

/// One bucket, as the trim ranks it.
struct Bucket {
    severity: u8,
    abs_z: f64,
    id: Uuid,
    status: String,
    /// Only used to break a `|z|` tie the same way the client does — it picks
    /// the peak by scanning buckets in `period_start` order and keeping the
    /// first maximum, so the earliest wins. Breaking the tie differently here
    /// would trim the bucket the row is about to describe.
    period_start: chrono::DateTime<chrono::FixedOffset>,
}

impl Bucket {
    /// Ordering key for "best of its status" — the same two axes the trim
    /// ranks on, minus the id tiebreak (which only matters for determinism
    /// among equals, and `max_by`'s last-wins is deterministic here because the
    /// input order is).
    fn rank(&self) -> (u8, f64) {
        (self.severity, self.abs_z)
    }
}

/// The event key as the *client* builds it: `event_id` when there is one,
/// `ungrouped:<row id>` otherwise, mirroring `groupIntoEvents`.
///
/// Only the internal key is used for grouping, but `truncated_events` is read
/// by the client against its own keys — the one place the two key spaces meet,
/// and the one place a bare id would silently fail to match.
fn client_event_key(model: &metric_anomalies::Model) -> String {
    match model.event_id {
        Some(event_id) => event_id.to_string(),
        None => format!("ungrouped:{}", model.id),
    }
}

/// Safety-valve ceiling on buckets returned per requested event (see
/// `load_ranked_events`). Generous — real events span a handful of buckets;
/// this only guards against an unbounded chain, not normal sizing.
pub(super) const MAX_BUCKETS_PER_EVENT: usize = 50;

#[cfg(test)]
mod tests {
    use super::{MAX_BUCKETS_PER_EVENT, cap_buckets_per_event, metric_anomalies};
    use chrono::TimeZone;
    use uuid::Uuid;

    /// A row in event `event`, ordered by `period_start` within it.
    fn row(event: Option<u128>, id: u128, day: i64) -> metric_anomalies::Model {
        let at = chrono::Utc
            .timestamp_opt(day * 86_400, 0)
            .single()
            .expect("timestamp");
        metric_anomalies::Model {
            id: Uuid::from_u128(id),
            workspace_id: Uuid::nil(),
            measure: "sales".into(),
            time_dimension: "order_date".into(),
            granularity: "day".into(),
            period_start: at.into(),
            period_end: at.into(),
            observed: 0.0,
            expected: 0.0,
            lower_bound: 0.0,
            upper_bound: 0.0,
            z_score: 0.0,
            severity: "low".into(),
            status: "new".into(),
            label: None,
            dimension_key: String::new(),
            filters: None,
            explain_cache: None,
            explain_cached_at: None,
            event_id: event.map(Uuid::from_u128),
            cohort_id: None,
            cohort_deviation: None,
            cohort_label: None,
            seasonal_period: None,
            detected_at: at.into(),
            updated_at: at.into(),
        }
    }

    #[test]
    fn keeps_every_event_when_none_exceeds_the_cap() {
        let mut rows = vec![
            row(Some(1), 10, 0),
            row(Some(1), 11, 1),
            row(Some(2), 12, 0),
        ];
        cap_buckets_per_event(&mut rows);
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn trims_a_runaway_event_without_dropping_later_events() {
        // The regression this shape exists for: a page-wide budget would have
        // spent itself on event 1 and dropped event 2 entirely — and no other
        // page would ever serve event 2, because `offset` counts events.
        let mut rows: Vec<_> = (0..MAX_BUCKETS_PER_EVENT as u128 + 5)
            .map(|i| row(Some(1), 100 + i, i as i64))
            .collect();
        rows.push(row(Some(2), 900, 0));
        assert!(cap_buckets_per_event(&mut rows).is_empty() == false);

        assert_eq!(rows.len(), MAX_BUCKETS_PER_EVENT + 1);
        assert_eq!(
            rows.last().expect("the later event survives").event_id,
            Some(Uuid::from_u128(2))
        );
    }

    #[test]
    fn trimming_keeps_the_worst_severity_even_when_its_z_is_mild() {
        // Severity comes from headroom, not from z, so the one `high` breach in
        // a chain can sit below every `low` continuation bucket on |z| alone.
        // Losing it would badge a real collapse `low` while the page's own
        // ranking (MAX(severity)) still sorted the event to the top.
        let mut rows: Vec<_> = (0..MAX_BUCKETS_PER_EVENT as u128 + 5)
            .map(|i| {
                let mut r = row(Some(1), 100 + i, i as i64);
                r.z_score = -9.0;
                r.severity = "low".into();
                r
            })
            .collect();
        let breach_id = Uuid::from_u128(50_000);
        let mut breach = row(Some(1), 50_000, 0);
        breach.z_score = -3.1;
        breach.severity = "high".into();
        rows.insert(0, breach);

        assert_eq!(cap_buckets_per_event(&mut rows).len(), 1);

        assert_eq!(rows.len(), MAX_BUCKETS_PER_EVENT);
        assert!(
            rows.iter().any(|r| r.id == breach_id),
            "the severest bucket must survive, whatever its z-score"
        );
    }

    #[test]
    fn trimming_keeps_a_bucket_of_every_status_the_event_holds() {
        // A status-blind trim can hand back an event that looks wholly
        // dismissed. Outside the Dismissed tab such a row offers no action at
        // all — nothing but Explain — while the New badge goes on counting the
        // `new` buckets that were trimmed away.
        let mut rows: Vec<_> = (0..MAX_BUCKETS_PER_EVENT as u128 + 5)
            .map(|i| {
                let mut r = row(Some(1), 100 + i, i as i64);
                r.severity = "high".into();
                r.status = "dismissed".into();
                r.z_score = -9.0;
                r
            })
            .collect();
        let fresh_id = Uuid::from_u128(60_000);
        let mut fresh = row(Some(1), 60_000, 0);
        fresh.severity = "low".into();
        fresh.status = "new".into();
        fresh.z_score = -0.5;
        rows.push(fresh);

        assert_eq!(cap_buckets_per_event(&mut rows).len(), 1);

        assert_eq!(rows.len(), MAX_BUCKETS_PER_EVENT);
        assert!(
            rows.iter().any(|r| r.id == fresh_id),
            "the row has to be able to tell this event still holds a `new` bucket"
        );
    }

    #[test]
    fn trimming_keeps_the_global_peak_even_when_a_whole_tier_outranks_it() {
        // The row's period, observed, expected and Δ% all come from the client's
        // peak — the global max |z|, whatever its severity. A severity-first
        // trim would drop it here and leave the row describing a milder bucket
        // under a correct badge.
        let mut rows: Vec<_> = (0..MAX_BUCKETS_PER_EVENT as u128 + 5)
            .map(|i| {
                let mut r = row(Some(1), 100 + i, i as i64);
                r.z_score = -2.0;
                r.severity = "high".into();
                r
            })
            .collect();
        let peak_id = Uuid::from_u128(50_000);
        let mut peak = row(Some(1), 50_000, 0);
        peak.z_score = -40.0;
        peak.severity = "low".into();
        rows.insert(0, peak);

        assert_eq!(cap_buckets_per_event(&mut rows).len(), 1);

        assert_eq!(rows.len(), MAX_BUCKETS_PER_EVENT);
        assert!(
            rows.iter().any(|r| r.id == peak_id),
            "the bucket the row's numbers come from must survive"
        );
    }

    #[test]
    fn trimming_keeps_the_buckets_that_define_the_row() {
        // The peak sits at the END of the chain, where a date-ordered trim
        // would have cut. The client reads peak and severity off what it
        // receives, so losing this bucket would make the row describe a
        // different problem.
        let mut rows: Vec<_> = (0..MAX_BUCKETS_PER_EVENT as u128 + 5)
            .map(|i| {
                let mut r = row(Some(1), 100 + i, i as i64);
                r.z_score = -1.0;
                r
            })
            .collect();
        let peak_id = Uuid::from_u128(100 + MAX_BUCKETS_PER_EVENT as u128 + 4);
        rows.last_mut().expect("a last bucket").z_score = -42.0;

        assert_eq!(cap_buckets_per_event(&mut rows).len(), 1);

        assert_eq!(rows.len(), MAX_BUCKETS_PER_EVENT);
        assert!(
            rows.iter().any(|r| r.id == peak_id),
            "the worst bucket must survive the trim"
        );
        // Still oldest-first: the trim filters, it does not reorder.
        assert!(
            rows.windows(2)
                .all(|w| w[0].period_start <= w[1].period_start)
        );
    }

    #[test]
    fn reports_trimmed_events_in_the_clients_key_space() {
        // `truncated_events` is the one place the server's grouping key and the
        // client's meet. A pre-event row keys `ungrouped:<id>` there, and a
        // bare id would silently match nothing.
        let mut rows: Vec<_> = (0..MAX_BUCKETS_PER_EVENT as u128 + 2)
            .map(|i| row(Some(7), 100 + i, i as i64))
            .collect();
        rows.push(row(None, 900, 0));

        let truncated = cap_buckets_per_event(&mut rows);

        assert_eq!(truncated, vec![Uuid::from_u128(7).to_string()]);
    }

    #[test]
    fn counts_ungrouped_rows_as_events_of_their_own() {
        // Pre-event rows key off their own id, so a run of them must not be
        // read as one huge event and trimmed.
        let mut rows: Vec<_> = (0..MAX_BUCKETS_PER_EVENT as u128 + 3)
            .map(|i| row(None, 200 + i, 0))
            .collect();
        let before = rows.len();
        assert!(cap_buckets_per_event(&mut rows).is_empty());
        assert_eq!(rows.len(), before);
    }
}
