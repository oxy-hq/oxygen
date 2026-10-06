//! `.monitor.yml` config files — one per workspace, declaring which
//! measures get anomaly-monitored.
//!
//! A single workspace's monitor config is a list of [`MonitorEntry`], each
//! pointing at a measure + time-dimension + granularity in the semantic model.
//! Decoupled from airlayer's `Measure` schema so we don't have to land an
//! upstream PR for an experimental field.
//!
//! Example:
//!
//! ```yaml
//! # workspaces/.../.monitor.yml
//! monitors:
//!   - measure: orders.revenue
//!     time_dimension: orders.created_at
//!     granularity: day
//!     lookback_days: 90
//!     seasonality: [7, 30]
//!     sensitivity: medium
//!
//!   - measure: orders.refund_rate
//!     time_dimension: orders.created_at
//!     granularity: week
//!     lookback_days: 365
//!     seasonality: [4]   # ~monthly cycle in week buckets
//! ```

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Per-granularity cron expressions read from `.monitor.yml`.
/// All fields are optional — only declared granularities get schedule rows.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct MonitorScheduleConfig {
    /// Cron expression for day-granularity monitors (e.g. `"0 6 * * *"`).
    pub daily: Option<String>,
    /// Cron expression for week-granularity monitors (e.g. `"0 6 * * 1"`).
    pub weekly: Option<String>,
    /// Cron expression for month-granularity monitors (e.g. `"5 6 1 * *"`).
    pub monthly: Option<String>,
}

/// All monitor entries declared for a workspace.
///
/// Loaded from a single `.monitor.yml` file under the workspace's semantics
/// scan path. Empty file / missing file → empty config (monitoring is opt-in).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MonitorConfig {
    /// Per-granularity cron schedule. Absent = no automated scanning.
    #[serde(default)]
    pub schedule: Option<MonitorScheduleConfig>,
    /// IANA timezone name (e.g. `America/Los_Angeles`) used to bucket every
    /// monitor in this file. Absent = UTC. Overridable per entry.
    #[serde(default)]
    pub timezone: Option<String>,
    /// Freshness watermark: withhold any bucket newer than this. Absent = zero.
    /// Overridable per entry.
    #[serde(default, with = "duration_opt")]
    pub freshness: Option<std::time::Duration>,
    /// Week-bucket start day. Absent = Monday. Overridable per entry.
    #[serde(default)]
    pub week_start: Option<WeekStart>,
    /// Named dates, surfaced as a label on a cohort that lands on one.
    ///
    /// A label, never a filter. There is deliberately no `exclude_dates:`:
    /// suppressing a date makes the monitor blind on exactly the days when
    /// unusual things happen, and a holiday with one store that *also* had an
    /// outage is a holiday cohort with one deviant member — which is the
    /// entire point of `cohort_deviation`.
    ///
    /// Tenant-supplied rather than from a holiday library: the meaningful
    /// calendar is per country, per industry and per tenant, and a restaurant
    /// chain's includes local events no library carries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar: Option<std::collections::HashMap<chrono::NaiveDate, String>>,
    /// Where a scan announces the insights it newly found. Absent = nowhere;
    /// the Insights Inbox is then the only place they appear.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify: Option<crate::notify::NotifyConfig>,
    #[serde(default)]
    pub monitors: Vec<MonitorEntry>,
}

impl MonitorConfig {
    /// Push file-level defaults into any entry that didn't override them, so
    /// each [`MonitorEntry`] is self-contained downstream — entries are cloned
    /// and fanned out by `group_by` expansion, detached from this config.
    pub fn apply_defaults(&mut self) {
        // Bind first: iterating `&mut self.monitors` while reading `self.*`
        // would be a double borrow.
        let (tz, freshness, week_start) = (self.timezone.clone(), self.freshness, self.week_start);
        for m in &mut self.monitors {
            if m.timezone.is_none() {
                m.timezone = tz.clone();
            }
            if m.freshness.is_none() {
                m.freshness = freshness;
            }
            if m.week_start.is_none() {
                m.week_start = week_start;
            }
        }
    }
}

/// A dimension filter applied when fetching the monitor's time series.
/// Corresponds to an airlayer `equals` filter on a single member.
///
/// Example (YAML):
/// ```yaml
/// filters:
///   - member: sales_daily.restaurant_id
///     values: ["loc-abc123"]
/// ```
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct MonitorFilter {
    /// Fully-qualified dimension id, e.g. `"sales_daily.restaurant_id"`.
    pub member: String,
    /// Values to match (OR within a single filter, AND across filters).
    pub values: Vec<String>,
}

impl MonitorFilter {
    /// Stable string key suitable for use in a unique index.
    /// Format: `member=v1,v2;member2=v3` (members sorted).
    pub fn key_for(filters: &[MonitorFilter]) -> String {
        if filters.is_empty() {
            return String::new();
        }
        let mut pairs: Vec<String> = filters
            .iter()
            .map(|f| {
                let mut vals = f.values.clone();
                vals.sort();
                format!("{}={}", f.member, vals.join(","))
            })
            .collect();
        pairs.sort();
        pairs.join(";")
    }
}

/// A single (measure, time-dim, granularity) tuple to monitor.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MonitorEntry {
    /// Fully qualified measure id, e.g. `"orders.revenue"`. Must exist in
    /// the workspace's semantic model at scan time.
    pub measure: String,
    /// Fully qualified time-dimension id used to bucket the series.
    pub time_dimension: String,
    /// Aggregation grain — `day`, `week`, `month`. The detection lookback
    /// is sampled at this grain.
    #[serde(default = "default_granularity")]
    pub granularity: Granularity,
    /// How many days of history to fit the forecast against. Default 90 for
    /// daily, but the loader bumps this implicitly when granularity is
    /// coarser so MSTL has enough cycles.
    #[serde(default = "default_lookback_days")]
    pub lookback_days: u32,
    /// Seasonal periods (in units of `granularity`) to decompose against.
    /// Defaults to `[7]` (weekly) for daily series; `[4]` for weekly;
    /// `[12]` for monthly. Override to add daily-of-month / yearly etc.
    #[serde(default)]
    pub seasonality: Option<Vec<usize>>,
    /// Detection threshold — controls how many σ the residual must exceed
    /// the forecast band before we flag it. Maps to a z-score cutoff
    /// inside the detector.
    #[serde(default)]
    pub sensitivity: Sensitivity,
    /// Optional human label used in the inbox UI. Falls back to the
    /// measure id when absent.
    #[serde(default)]
    pub label: Option<String>,
    /// Optional dimension filters applied when fetching the series.
    /// Use this to monitor a single store / segment rather than the
    /// chain-wide aggregate.
    #[serde(default)]
    pub filters: Vec<MonitorFilter>,
    /// Fan-out dimension: instead of writing one entry per segment value,
    /// declare the dimension here and the scanner discovers all active
    /// values at scan time and runs one detector per segment automatically.
    ///
    /// Example:
    /// ```yaml
    /// group_by: sales_daily.restaurant_id
    /// ```
    ///
    /// `group_by` is applied in addition to any `filters` already declared
    /// (AND semantics). For example, `filters: [{region: US}]` combined with
    /// `group_by: restaurant_id` monitors each US restaurant individually.
    #[serde(default)]
    pub group_by: Option<String>,
    /// Which direction of deviation to flag. Defaults to `both`.
    /// Use `decrease` for metrics where only drops matter (revenue, order
    /// volume, pipeline row counts) and `increase` for metrics where only
    /// spikes matter (error rates, latency, costs).
    #[serde(default)]
    pub direction: Direction,
    /// Overrides the file-level `timezone`.
    #[serde(default)]
    pub timezone: Option<String>,
    /// Overrides the file-level `freshness`.
    #[serde(default, with = "duration_opt")]
    pub freshness: Option<std::time::Duration>,
    /// Overrides the file-level `week_start`.
    #[serde(default)]
    pub week_start: Option<WeekStart>,
}

/// Which direction of anomaly to surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// Flag both unexpected drops and unexpected spikes (default).
    #[default]
    Both,
    /// Flag only when the observed value is below the expected (drops).
    Decrease,
    /// Flag only when the observed value is above the expected (spikes).
    Increase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Granularity {
    Day,
    Week,
    Month,
}

impl Granularity {
    /// String matching airlayer's `TimeDimensionQuery::granularity` field.
    pub fn airlayer_str(self) -> &'static str {
        match self {
            Granularity::Day => "day",
            Granularity::Week => "week",
            Granularity::Month => "month",
        }
    }

    /// Default seasonal periods when the user doesn't override.
    /// Single-period defaults — daily gets weekly, weekly gets monthly-ish,
    /// monthly gets yearly. Multi-seasonality (e.g. day + week + year) is
    /// available by overriding `seasonality` in the YAML.
    pub fn default_seasonality(self) -> Vec<usize> {
        match self {
            Granularity::Day => vec![7],
            Granularity::Week => vec![4],
            Granularity::Month => vec![12],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Sensitivity {
    /// Flag only large deviations (~5σ). Fewer false positives, may miss
    /// subtle moves.
    Low,
    /// Default. Flag at ~3σ.
    #[default]
    Medium,
    /// Flag at ~2σ. Noisier; useful for dashboards under active investigation.
    High,
}

impl Sensitivity {
    /// Residual z-score cutoff. Detector flags when `|residual| > σ * cutoff`.
    pub fn z_cutoff(self) -> f64 {
        match self {
            Sensitivity::Low => 5.0,
            Sensitivity::Medium => 3.0,
            Sensitivity::High => 2.0,
        }
    }
}

/// Which weekday a `granularity: week` bucket starts on.
///
/// This aligns **Oxy's own** period boundaries (the scan window's
/// `period_start`); it is *not* sent to the warehouse, which labels its week
/// buckets using its own dialect rule. Set it to match your dialect — verified
/// against the pinned airlayer revision (`474f461`, `src/dialect/mod.rs:36-77`):
///
/// | Dialect | week truncation | starts on |
/// | --- | --- | --- |
/// | ClickHouse | `toMonday(...)` | Monday |
/// | Postgres / DuckDB / Redshift / Trino | `date_trunc('week', ...)` | Monday |
/// | Snowflake / Presto | `DATE_TRUNC('week', ...)` | Monday |
/// | BigQuery | `TIMESTAMP_TRUNC(x, WEEK)` | Sunday |
/// | MySQL / Domo | `DATE_SUB(x, INTERVAL DAYOFWEEK(x)-1 DAY)` | Sunday |
///
/// Monday is therefore the default (it matches every dialect but BigQuery and
/// MySQL, and reproduces the pre-timezone hardcoded behavior exactly). A
/// BigQuery- or MySQL-backed weekly monitor wants `week_start: sunday`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WeekStart {
    Sunday,
    #[default]
    Monday,
}

impl WeekStart {
    /// Days from Monday, matching `chrono::Weekday::num_days_from_monday`.
    pub fn num_days_from_monday(self) -> i64 {
        match self {
            WeekStart::Sunday => 6,
            WeekStart::Monday => 0,
        }
    }
}

/// `freshness` is written as a humantime duration (`30m`, `6h`, `3d`, `1w`,
/// `1d 12h`). Note humantime's `m` is minutes and `M` is months, and its month
/// is an average 30.44 days — express anything above a week in days or weeks.
/// `humantime` (de)serialization for `Option<Duration>` — the `3d` / `30m` wire
/// format. Public because `reconcile.yml` must parse the same strings the same
/// way; two copies would drift.
pub mod duration_opt {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn deserialize<'de, D>(d: D) -> Result<Option<Duration>, D::Error>
    where
        D: Deserializer<'de>,
    {
        match Option::<String>::deserialize(d)? {
            None => Ok(None),
            Some(s) => humantime::parse_duration(&s)
                .map(Some)
                .map_err(serde::de::Error::custom),
        }
    }

    pub fn serialize<S>(v: &Option<Duration>, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match v {
            None => s.serialize_none(),
            Some(d) => s.serialize_str(&humantime::format_duration(*d).to_string()),
        }
    }
}

fn default_granularity() -> Granularity {
    Granularity::Day
}

fn default_lookback_days() -> u32 {
    90
}

/// Failure modes when loading a monitor config.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("monitor config not found at {0}")]
    NotFound(PathBuf),
    #[error("workspace directory {0} is not on this node, so whether it has monitors is unknown")]
    WorkspaceAbsent(PathBuf),
    #[error("failed to read monitor config at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid monitor config at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_yaml::Error,
    },
    #[error("invalid IANA timezone {name:?} in monitor config at {path}")]
    InvalidTimezone { path: PathBuf, name: String },
    #[error(
        "notify.slack_channel {value:?} in monitor config at {path} is not a Slack channel id \
         (it looks like C0123ABCDEF; copy it from the channel's details in Slack)"
    )]
    InvalidNotifyChannel { path: PathBuf, value: String },
    #[error("invalid monitor config: {0}")]
    Definition(#[source] serde_json::Error),
}

/// Load a single `.monitor.yml` from disk. A missing file returns an empty
/// config — monitoring is opt-in, so its absence is not an error.
///
/// A missing *directory* is a different answer. It means this process does not
/// hold the workspace, and reporting "no monitors" for it is how anomaly
/// detection came to switch off silently on the stateless fleet: the scan
/// endpoint returned `200 {"monitors_scanned": 0}` and recorded the run DONE.
/// The opt-in contract only makes sense once you know you are looking at the
/// right tree.
pub fn load_from_file(path: &Path) -> Result<MonitorConfig, LoadError> {
    if !path.exists() {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
            && !parent.is_dir()
        {
            return Err(LoadError::WorkspaceAbsent(parent.to_path_buf()));
        }
        return Ok(MonitorConfig::default());
    }
    let text = std::fs::read_to_string(path).map_err(|source| LoadError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if text.trim().is_empty() {
        return Ok(MonitorConfig::default());
    }
    let cfg: MonitorConfig = serde_yaml::from_str(&text).map_err(|source| LoadError::Parse {
        path: path.to_path_buf(),
        source,
    })?;

    finish(cfg, path)
}

/// Parse a config that has already been read into JSON — the compile
/// boundary's `monitor_configs.definition`. Runs the same checks and applies
/// the same defaults as [`load_from_file`], so an entry taken from the boundary
/// is the entry a scan of the file would run.
pub fn from_definition(definition: serde_json::Value) -> Result<MonitorConfig, LoadError> {
    let cfg = serde_json::from_value(definition).map_err(LoadError::Definition)?;
    finish(cfg, Path::new(".monitor.yml"))
}

/// What every source of a config goes through after it parses: the checks a
/// type cannot make, then the file-level defaults.
fn finish(mut cfg: MonitorConfig, path: &Path) -> Result<MonitorConfig, LoadError> {
    // Validate BEFORE resolving defaults so the error names the field the user
    // actually wrote, and so a bad name can never silently degrade to UTC.
    let declared = cfg
        .timezone
        .iter()
        .chain(cfg.monitors.iter().filter_map(|m| m.timezone.as_ref()));
    for name in declared {
        if name.parse::<chrono_tz::Tz>().is_err() {
            return Err(LoadError::InvalidTimezone {
                path: path.to_path_buf(),
                name: name.clone(),
            });
        }
    }

    if let Some(notify) = cfg.notify.as_ref().filter(|n| !n.has_channel_id()) {
        return Err(LoadError::InvalidNotifyChannel {
            path: path.to_path_buf(),
            value: notify.slack_channel.clone(),
        });
    }

    cfg.apply_defaults();
    Ok(cfg)
}

/// Convenience: the conventional location of the config inside a workspace
/// (`<workspace>/.monitor.yml`). Callers may also accept user-provided paths.
pub fn default_config_path(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".monitor.yml")
}

impl MonitorEntry {
    /// One segment of this `group_by` entry: the entry narrowed to a single
    /// value of the fan-out dimension.
    ///
    /// The value is ANDed with any filters already declared, so
    /// `filters: [{region: US}]` + `group_by: restaurant_id` scans each
    /// restaurant scoped to US only. The result carries no `group_by` of its
    /// own — it is a plain entry, and is scanned as one.
    pub fn segment_for(&self, dimension: &str, value: String) -> MonitorEntry {
        let mut segment = self.clone();
        segment.group_by = None;
        segment.filters.push(MonitorFilter {
            member: dimension.to_string(),
            values: vec![value],
        });
        segment
    }

    /// Resolved seasonal periods — explicit override wins over the
    /// granularity default.
    pub fn effective_seasonality(&self) -> Vec<usize> {
        self.seasonality
            .clone()
            .unwrap_or_else(|| self.granularity.default_seasonality())
    }

    /// Resolved bucketing timezone. Defaults to UTC. The name is validated at
    /// load time, so an unparseable value here cannot reach production.
    pub fn effective_timezone(&self) -> chrono_tz::Tz {
        self.timezone
            .as_deref()
            .and_then(|s| s.parse::<chrono_tz::Tz>().ok())
            .unwrap_or(chrono_tz::UTC)
    }

    /// Resolved freshness watermark. Defaults to zero, which reproduces the
    /// pre-freshness behavior exactly.
    pub fn effective_freshness(&self) -> chrono::Duration {
        self.freshness
            .and_then(|d| chrono::Duration::from_std(d).ok())
            .unwrap_or_else(chrono::Duration::zero)
    }

    /// Resolved week-bucket start day. Defaults to Monday, which reproduces
    /// the pre-timezone hardcoded boundary.
    pub fn effective_week_start(&self) -> WeekStart {
        self.week_start.unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_entry() {
        let yaml = r#"
monitors:
  - measure: orders.revenue
    time_dimension: orders.created_at
"#;
        let cfg: MonitorConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.monitors.len(), 1);
        let m = &cfg.monitors[0];
        assert_eq!(m.measure, "orders.revenue");
        assert_eq!(m.granularity, Granularity::Day);
        assert_eq!(m.lookback_days, 90);
        assert_eq!(m.sensitivity, Sensitivity::Medium);
        assert_eq!(m.effective_seasonality(), vec![7]);
    }

    #[test]
    fn parses_a_calendar_of_named_dates() {
        let yaml = r#"
calendar:
  2025-07-04: Independence Day
  2025-11-27: Thanksgiving

monitors:
  - measure: orders.revenue
    time_dimension: orders.created_at
"#;
        let cfg: MonitorConfig = serde_yaml::from_str(yaml).unwrap();
        let cal = cfg.calendar.expect("calendar parsed");
        assert_eq!(
            cal.get(&chrono::NaiveDate::from_ymd_opt(2025, 7, 4).unwrap())
                .map(String::as_str),
            Some("Independence Day")
        );
    }

    #[test]
    fn rejects_unknown_fields() {
        let yaml = r#"
monitors:
  - measure: orders.revenue
    time_dimension: orders.created_at
    unknown_field: 42
"#;
        assert!(serde_yaml::from_str::<MonitorConfig>(yaml).is_err());
    }

    #[test]
    fn sensitivity_z_cutoffs() {
        assert!(Sensitivity::Low.z_cutoff() > Sensitivity::Medium.z_cutoff());
        assert!(Sensitivity::Medium.z_cutoff() > Sensitivity::High.z_cutoff());
    }

    #[test]
    fn parses_schedule_block() {
        let yaml = r#"
schedule:
  daily: "0 6 * * *"
  weekly: "0 6 * * 1"

monitors:
  - measure: orders.revenue
    time_dimension: orders.created_at
"#;
        let cfg: MonitorConfig = serde_yaml::from_str(yaml).unwrap();
        let sched = cfg.schedule.unwrap();
        assert_eq!(sched.daily.as_deref(), Some("0 6 * * *"));
        assert_eq!(sched.weekly.as_deref(), Some("0 6 * * 1"));
        assert!(sched.monthly.is_none());
    }

    #[test]
    fn schedule_block_optional() {
        let yaml = r#"
monitors:
  - measure: orders.revenue
    time_dimension: orders.created_at
"#;
        let cfg: MonitorConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(cfg.schedule.is_none());
    }

    #[test]
    fn parses_timezone_freshness_and_week_start() {
        let yaml = r#"
timezone: America/Los_Angeles
freshness: 3d
week_start: monday

monitors:
  - measure: orders.revenue
    time_dimension: orders.created_at
"#;
        let cfg: MonitorConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.timezone.as_deref(), Some("America/Los_Angeles"));
        assert_eq!(
            cfg.freshness,
            Some(std::time::Duration::from_secs(3 * 86_400))
        );
        assert_eq!(cfg.week_start, Some(WeekStart::Monday));
    }

    #[test]
    fn file_level_defaults_flow_into_entries() {
        let yaml = r#"
timezone: America/Los_Angeles
freshness: 3d

monitors:
  - measure: orders.revenue
    time_dimension: orders.created_at
  - measure: eu.revenue
    time_dimension: eu.created_at
    timezone: Europe/Berlin
    freshness: 0s
"#;
        let mut cfg: MonitorConfig = serde_yaml::from_str(yaml).unwrap();
        cfg.apply_defaults();

        // Inherits the file-level default.
        assert_eq!(
            cfg.monitors[0].effective_timezone(),
            chrono_tz::America::Los_Angeles
        );
        assert_eq!(
            cfg.monitors[0].effective_freshness(),
            chrono::Duration::days(3)
        );

        // Per-entry override wins, including an explicit zero.
        assert_eq!(
            cfg.monitors[1].effective_timezone(),
            chrono_tz::Europe::Berlin
        );
        assert_eq!(
            cfg.monitors[1].effective_freshness(),
            chrono::Duration::zero()
        );
    }

    #[test]
    fn absent_fields_mean_utc_zero_monday() {
        let yaml = r#"
monitors:
  - measure: orders.revenue
    time_dimension: orders.created_at
"#;
        let mut cfg: MonitorConfig = serde_yaml::from_str(yaml).unwrap();
        cfg.apply_defaults();
        let m = &cfg.monitors[0];
        assert_eq!(m.effective_timezone(), chrono_tz::UTC);
        assert_eq!(m.effective_freshness(), chrono::Duration::zero());
        assert_eq!(m.effective_week_start(), WeekStart::Monday);
    }

    #[test]
    fn accepts_humantime_duration_forms() {
        for (text, expected_secs) in [
            ("30m", 1_800u64),
            ("6h", 21_600),
            ("3d", 259_200),
            ("1w", 604_800),
            ("1d 12h", 129_600),
        ] {
            let yaml = format!(
                "freshness: {text}\nmonitors:\n  - measure: a.b\n    time_dimension: a.t\n"
            );
            let cfg: MonitorConfig = serde_yaml::from_str(&yaml)
                .unwrap_or_else(|e| panic!("{text:?} should parse: {e}"));
            assert_eq!(
                cfg.freshness,
                Some(std::time::Duration::from_secs(expected_secs)),
                "freshness {text:?}"
            );
        }
    }

    #[test]
    fn rejects_unparseable_duration() {
        let yaml = r#"
freshness: "not a duration"
monitors:
  - measure: a.b
    time_dimension: a.t
"#;
        assert!(serde_yaml::from_str::<MonitorConfig>(yaml).is_err());
    }

    /// The compile boundary hands the config over as JSON. It has to come out
    /// the same as the file would: defaults applied, and the same refusals.
    #[test]
    fn a_definition_from_the_boundary_gets_the_files_defaults_and_checks() {
        let definition = serde_json::json!({
            "timezone": "America/Los_Angeles",
            "monitors": [{ "measure": "a.b", "time_dimension": "a.t" }],
        });
        let cfg = from_definition(definition).unwrap();
        assert_eq!(
            cfg.monitors[0].effective_timezone(),
            chrono_tz::America::Los_Angeles
        );

        let bad_zone = serde_json::json!({ "timezone": "Mars/Olympus_Mons", "monitors": [] });
        assert!(matches!(
            from_definition(bad_zone),
            Err(LoadError::InvalidTimezone { .. })
        ));
        let unknown = serde_json::json!({ "monitors": [], "notfiy": {} });
        assert!(matches!(
            from_definition(unknown),
            Err(LoadError::Definition(_))
        ));
    }

    #[test]
    fn rejects_unknown_timezone_at_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".monitor.yml");
        std::fs::write(
            &path,
            "timezone: Mars/Olympus_Mons\nmonitors:\n  - measure: a.b\n    time_dimension: a.t\n",
        )
        .unwrap();
        let err = load_from_file(&path).expect_err("unknown tz must not silently fall back to UTC");
        assert!(
            matches!(err, LoadError::InvalidTimezone { .. }),
            "expected InvalidTimezone, got {err:?}"
        );
    }
}

#[cfg(test)]
mod absence_tests {
    use super::*;

    /// Monitoring is opt-in, so a workspace with no `.monitor.yml` has no
    /// monitors. That is a real answer and must keep working.
    #[test]
    fn a_missing_file_in_a_real_workspace_is_still_opt_out() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = load_from_file(&dir.path().join(".monitor.yml"))
            .expect("an absent config in a present workspace is not an error");
        assert!(cfg.monitors.is_empty());
    }

    /// A missing *directory* is a different answer. Reporting "no monitors" for
    /// a workspace this node does not hold is how the scan endpoint came to
    /// return `200 {"monitors_scanned": 0}` and record the run DONE.
    #[test]
    fn a_missing_workspace_directory_is_not_opt_out() {
        let dir = tempfile::tempdir().unwrap();
        let absent = dir.path().join("never-cloned-here");
        let err = load_from_file(&absent.join(".monitor.yml"))
            .expect_err("an absent workspace must not read as an opted-out one");
        assert!(matches!(err, LoadError::WorkspaceAbsent(_)), "{err:?}");
    }
}
