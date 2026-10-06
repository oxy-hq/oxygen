//! A dry run of one monitor: what the next scan would make of it, with
//! nothing written.
//!
//! "Scan now" is the only way to find out what a `.monitor.yml` entry does,
//! and it answers by filing into the Insights Inbox — and, with a `notify:`
//! block, by posting to a channel. So the first time an author learns that a
//! measure does not resolve, that a segment has too little history to be
//! scored, or that a sensitivity flags half of last week, the answer is
//! already in front of everyone else.
//!
//! [`preview_monitor`] runs the same [`scan_one`] a scan runs, on one entry,
//! and returns what it found. It takes no database handle and has nowhere to
//! put a result. The window is the scan's own: the seven most recent buckets a
//! daily series has, the most recent one of a weekly or monthly. "Most recent"
//! means of the data, not of the calendar — on a measure whose warehouse table
//! stopped in July, a preview in October reports on a week in July, because
//! that is what a scan would score.

use std::sync::Arc;

use agentic_analytics::MetricTreeRunner;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::Instrument;

use crate::config::{Granularity, MonitorConfig, MonitorEntry, MonitorFilter};
use crate::detect::DetectedAnomaly;
use crate::service::{OpenEvents, ScanError, SegmentKey, default_test_window, scan_one};

#[cfg(test)]
mod tests;

/// How many segments of a `group_by` monitor a preview scans. A scan fits
/// every segment; a preview is someone waiting on a request, and the first
/// dozen answer "does this entry work, and how noisy is it" well enough. The
/// response says how many there were, so the rest are not silently assumed.
pub const MAX_PREVIEW_SEGMENTS: usize = 12;

/// Names one entry of the file: the triple the scanner keys on, plus the two
/// things that tell apart entries sharing it — the entry's own filters, and
/// whether it fans out.
#[derive(Debug, Clone, Deserialize)]
pub struct MonitorSelector {
    pub measure: String,
    pub time_dimension: String,
    pub granularity: Granularity,
    /// [`MonitorFilter::key_for`] of the entry's `filters`; empty when it has
    /// none. What tells two entries over one measure apart.
    #[serde(default)]
    pub dimension_key: String,
    /// The entry's `group_by`, if it has one. A file commonly declares a total
    /// and the same measure split per location; the two differ in nothing
    /// else, and without this the split's preview would be the total's.
    #[serde(default)]
    pub group_by: Option<String>,
}

impl MonitorSelector {
    /// The entry this names, if the file still has it.
    pub fn find<'a>(&self, config: &'a MonitorConfig) -> Option<&'a MonitorEntry> {
        config.monitors.iter().find(|m| {
            m.measure == self.measure
                && m.time_dimension == self.time_dimension
                && m.granularity == self.granularity
                && MonitorFilter::key_for(&m.filters) == self.dimension_key
                && m.group_by == self.group_by
        })
    }
}

/// What a scan would make of one monitor right now.
#[derive(Debug, Serialize)]
pub struct MonitorPreview {
    /// How many of the most recent buckets a scan of this grain scores.
    pub window_buckets: usize,
    /// Segments the entry fans out to; `1` without `group_by`.
    pub segments_total: usize,
    /// The segments that were scanned — all of them, or the first
    /// [`MAX_PREVIEW_SEGMENTS`].
    pub segments: Vec<SegmentPreview>,
}

#[derive(Debug, Serialize)]
pub struct SegmentPreview {
    /// Empty for a monitor with no filters and no `group_by`.
    pub dimension_key: String,
    #[serde(flatten)]
    pub outcome: SegmentOutcome,
}

/// The three things a scan can make of a segment. Kept apart on purpose: an
/// empty `flagged` means "looked, found nothing" only under `Scored`.
#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SegmentOutcome {
    Scored {
        measured_buckets: usize,
        required_buckets: usize,
        flagged: Vec<DetectedAnomaly>,
    },
    /// Too little history to be scored at all, so nothing can be flagged.
    WarmingUp {
        measured_buckets: usize,
        required_buckets: usize,
    },
    Failed {
        error: String,
    },
}

/// Scan `entry` once, as the next scan would, and report without persisting.
///
/// `open_events` is what a scan is handed too: a bucket that continues an
/// event already on record is kept where it would otherwise be dropped, and
/// leaving that out would make the preview disagree with the scan it stands
/// in for.
///
/// Fails as a whole only when a `group_by` dimension's values cannot be
/// listed — there are then no segments to report on. A segment that errors is
/// reported as that segment's outcome.
pub async fn preview_monitor(
    runner: Arc<dyn MetricTreeRunner>,
    entry: &MonitorEntry,
    now: DateTime<Utc>,
    open_events: &OpenEvents,
) -> Result<MonitorPreview, ScanError> {
    let all = segments_of(runner.as_ref(), entry).await?;
    let segments_total = all.len();

    let mut set = tokio::task::JoinSet::new();
    for (index, segment) in all.into_iter().take(MAX_PREVIEW_SEGMENTS).enumerate() {
        let runner = runner.clone();
        let continuation = open_events.get(&SegmentKey::for_entry(&segment)).copied();
        set.spawn(
            async move {
                let scan = scan_one(runner, &segment, now, continuation).await;
                let outcome = match scan {
                    Ok(scan) if scan.coverage.is_warming_up() => SegmentOutcome::WarmingUp {
                        measured_buckets: scan.coverage.measured,
                        required_buckets: scan.coverage.required,
                    },
                    Ok(scan) => SegmentOutcome::Scored {
                        measured_buckets: scan.coverage.measured,
                        required_buckets: scan.coverage.required,
                        flagged: scan.anomalies,
                    },
                    Err(error) => SegmentOutcome::Failed {
                        error: error.to_string(),
                    },
                };
                let dimension_key = MonitorFilter::key_for(&segment.filters);
                (
                    index,
                    SegmentPreview {
                        dimension_key,
                        outcome,
                    },
                )
            }
            // A spawned task starts with no span; without this each segment's
            // `monitor_scan_one` is an orphan root instead of a child of the request.
            .in_current_span(),
        );
    }

    // Tasks finish in any order; the response keeps the order the segments
    // were discovered in, so two previews of one monitor read the same.
    let mut segments = Vec::new();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(done) => segments.push(done),
            Err(e) => {
                tracing::error!(target: "metric_monitoring", error = %e, "preview task panicked")
            }
        }
    }
    segments.sort_by_key(|(index, _)| *index);

    Ok(MonitorPreview {
        window_buckets: default_test_window(entry.granularity),
        segments_total,
        segments: segments.into_iter().map(|(_, s)| s).collect(),
    })
}

/// The entry itself, or one entry per value of its `group_by` dimension — the
/// same fan-out a scan performs, with the same filter semantics.
async fn segments_of(
    runner: &dyn MetricTreeRunner,
    entry: &MonitorEntry,
) -> Result<Vec<MonitorEntry>, ScanError> {
    let Some(dimension) = entry.group_by.as_ref() else {
        return Ok(vec![entry.clone()]);
    };
    let values = runner
        .get_dimension_values(
            dimension.clone(),
            entry.measure.clone(),
            entry.lookback_days,
        )
        .await
        .map_err(ScanError::FetchSeries)?;
    Ok(values
        .into_iter()
        .map(|value| entry.segment_for(dimension, value))
        .collect())
}
