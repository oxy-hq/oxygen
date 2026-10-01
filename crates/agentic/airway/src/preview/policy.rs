//! The submit-time rules ([`SamplePolicy`]) and the one refusal the host feeds
//! with what it learns offline ([`metadata_in_main`]).

use airway::connector::ResourceInfo;
use airway::types::WriteDisposition;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use super::{
    DEFAULT_WINDOW_DAYS, MAX_WINDOW_DAYS, SAMPLE_REFUSED_KINDS, SampleRefusal, is_windowed,
    rotates_on_use,
};
use crate::schema_compat::Schema;

/// A sample's date window, `[from, to)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SampleWindow {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

impl SampleWindow {
    /// The window as the executor's backfill pair (RFC 3339).
    pub fn as_backfill(&self) -> (String, String) {
        (self.from.to_rfc3339(), self.to.to_rfc3339())
    }
}

/// The window a request asked for; either bound may be missing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct RequestedWindow {
    #[serde(default)]
    pub from: Option<DateTime<Utc>>,
    #[serde(default)]
    pub to: Option<DateTime<Utc>>,
}

/// What a sample request says, with what the host could learn about the
/// source without calling it.
pub struct SampleAsk<'a> {
    pub pipeline: &'a str,
    pub kind: &'a str,
    pub window: Option<RequestedWindow>,
    pub resources: &'a [String],
    /// The resources the source advertises; `None` when the host could not
    /// tell without reaching the source (a sample must then name them).
    pub advertised: Option<&'a [String]>,
    /// A sandbox company is registered for this pipeline.
    pub has_sandbox: bool,
    pub now: DateTime<Utc>,
}

/// What an accepted sample runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SamplePlan {
    pub window: Option<SampleWindow>,
    pub resources: Vec<String>,
    /// No date window bounds it, so the host's wall-clock cap does.
    pub wall_clock_capped: bool,
}

/// The submit-time rules (module doc).
pub struct SamplePolicy;

impl SamplePolicy {
    /// Accept `ask` as a [`SamplePlan`], or say why not: a refused kind first,
    /// then a missing sandbox, then the window and the resources.
    pub fn check(ask: &SampleAsk<'_>) -> Result<SamplePlan, SampleRefusal> {
        if SAMPLE_REFUSED_KINDS.contains(&ask.kind) {
            return Err(SampleRefusal::Refused {
                kind: ask.kind.to_string(),
            });
        }
        if rotates_on_use(ask.kind) && !ask.has_sandbox {
            return Err(SampleRefusal::SandboxRequired {
                pipeline: ask.pipeline.to_string(),
            });
        }
        let windowed = is_windowed(ask.kind);
        let window = match (windowed, ask.window) {
            (true, requested) => Some(window_for(requested.unwrap_or_default(), ask.now)?),
            (false, Some(_)) => {
                return Err(SampleRefusal::WindowNotSupported {
                    kind: ask.kind.to_string(),
                });
            }
            (false, None) => None,
        };
        let resources = resources_for(windowed, ask.resources, ask.advertised)?;
        Ok(SamplePlan {
            window,
            resources,
            wall_clock_capped: !windowed,
        })
    }
}

/// The window a windowed sample runs: the last [`DEFAULT_WINDOW_DAYS`] when
/// none was asked for, otherwise both bounds, in order, at most
/// [`MAX_WINDOW_DAYS`] apart.
fn window_for(
    requested: RequestedWindow,
    now: DateTime<Utc>,
) -> Result<SampleWindow, SampleRefusal> {
    let (from, to) = match (requested.from, requested.to) {
        (None, None) => (now - Duration::days(DEFAULT_WINDOW_DAYS), now),
        (Some(from), Some(to)) => (from, to),
        _ => {
            return Err(SampleRefusal::WindowRequired(
                "a sample window needs both `from` and `to` (or neither, for the last 7 days)"
                    .into(),
            ));
        }
    };
    if from >= to {
        return Err(SampleRefusal::WindowRequired(
            "a sample window's `from` must be before its `to`".into(),
        ));
    }
    let span = to - from;
    if span > Duration::days(MAX_WINDOW_DAYS) {
        // Rounded up: 31 days and a minute is refused as 32.
        let days = (span.num_seconds() + 86_399) / 86_400;
        return Err(SampleRefusal::WindowTooLong { days });
    }
    Ok(SampleWindow { from, to })
}

/// The resources a sample reads. Every named one must be advertised; a sample
/// with no window must name them when the source has more than one (or when
/// the host could not tell how many it has).
fn resources_for(
    windowed: bool,
    requested: &[String],
    advertised: Option<&[String]>,
) -> Result<Vec<String>, SampleRefusal> {
    if let Some(advertised) = advertised
        && let Some(unknown) = requested.iter().find(|r| !advertised.contains(r))
    {
        return Err(SampleRefusal::UnknownResource {
            name: unknown.clone(),
            advertised: advertised.to_vec(),
        });
    }
    if windowed || !requested.is_empty() {
        return Ok(requested.to_vec());
    }
    // A source with no window reads named resources only, so a single one is
    // named for the caller: the claim-time check (`PreviewSample::apply`)
    // then never meets a capped sample with nothing named.
    match advertised {
        Some([only]) => Ok(vec![only.clone()]),
        _ => Err(SampleRefusal::ResourcesRequired {
            advertised: advertised.map(<[String]>::to_vec).unwrap_or_default(),
        }),
    }
}

/// The sampled tables whose load would write airway's metadata into `main`
/// (`main._aw_compaction_manifest`, `main._aw_table_watermarks`), which a
/// preview Writer confined to the preview's schemas cannot write: every
/// resource the source declares `replacing`, and every table production's
/// stored schema holds as `replacing` or with a watermark (business) column.
/// Only the resources the sample reads count (`sampled`; empty = all).
pub fn metadata_in_main(
    resources: &[ResourceInfo],
    stored: Option<&Schema>,
    sampled: &[String],
) -> Vec<String> {
    let read = |name: &str| sampled.is_empty() || sampled.iter().any(|s| s == name);
    let declared = resources
        .iter()
        .filter(|r| r.write_disposition == WriteDisposition::Replacing && read(&r.name))
        .map(|r| r.name.clone());
    let stored_tables = stored
        .into_iter()
        .flat_map(|s| s.tables.values())
        .filter(|t| {
            let root = t.parent.as_deref().unwrap_or(&t.name);
            (t.write_disposition == WriteDisposition::Replacing || t.business_column.is_some())
                && (read(&t.name) || read(root))
        });
    let mut tables: Vec<String> = declared
        .chain(stored_tables.map(|t| t.name.clone()))
        .collect();
    tables.sort();
    tables.dedup();
    tables
}
