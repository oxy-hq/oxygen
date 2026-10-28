//! Per-workspace health-check cadence, configured in `config.yml` under
//! `health_check:`. The cadence drives the workspace's `health_eval` schedule
//! row (see `agentic_pipeline::scheduler::reconcile_health_schedule`); the eval
//! itself runs as a `TaskSpec::Custom { kind: "health_eval_workspace" }` on the
//! global-run fleet. Operator-facing notes — who gets evaluated, and what else
//! rides inside the eval pass — live in `internal-docs/admin-surfaces.md`
//! ("Workspace health → who actually gets evaluated").
//!
//! **Health checks are off until a workspace asks for them.** Writing a
//! `health_check:` block is the opt-in; a workspace with no block runs no eval
//! at all. See [`resolve_enabled`] — that function, not the `enabled` field's
//! serde default, is the policy, because the two answer different questions:
//! the field default covers "block written, `enabled:` omitted" (→ on), while
//! `resolve_enabled` covers "no block at all" (→ off).

use std::collections::BTreeMap;
use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Lower bound (10m) — the dispatcher tick's effective floor; checking more
/// often than this can't fire sooner anyway.
const MIN_INTERVAL_SECS: u64 = 600;
/// Upper bound (24h).
const MAX_INTERVAL_SECS: u64 = 86_400;
/// Default cadence when `health_check` (or its `interval`) is absent.
const DEFAULT_INTERVAL_SECS: u64 = 3600;

/// Smoke-test cadence bounds. The floor matches the eval floor — the smoke run
/// is gated inside an eval pass, so it can never fire more often than one. The
/// ceiling is 7d: a smoke verdict older than a week is not worth surfacing.
const MIN_SMOKE_INTERVAL_SECS: u64 = 600;
const MAX_SMOKE_INTERVAL_SECS: u64 = 604_800;
/// Default smoke cadence (6h). Deliberately far slower than the eval cadence:
/// every probe costs a warehouse round-trip, and the agent probe costs tokens.
const DEFAULT_SMOKE_INTERVAL_SECS: u64 = 21_600;

/// Default cap on how many targets a single probe kind may exercise per run, so
/// a workspace with hundreds of topics can't hammer the warehouse. Truncation is
/// never silent — the runner emits a verdict recording what it skipped.
const DEFAULT_MAX_TARGETS: usize = 25;

// No `Eq`: app-probe `variables` hold arbitrary JSON values, which are only
// `PartialEq`. `PartialEq` is all any caller needs — `smoke_due` compares the
// stored config against the current one to decide whether to re-probe.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HealthCheckConfig {
    /// Evaluation cadence as a humantime duration (e.g. `"30m"`, `"2h"`).
    /// Absent → 1h. Out-of-range or unparseable values clamp/fall back to the
    /// safe default rather than erroring — a malformed cadence must never wedge
    /// the scheduler.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval: Option<String>,
    /// When false, the workspace's health schedule row is disabled and no eval
    /// is enqueued. Defaults to true *within a written block* — having gone to
    /// the trouble of writing `health_check:`, you meant to turn it on.
    ///
    /// This default says nothing about a workspace with **no** `health_check:`
    /// block: that one is off. Ask [`resolve_enabled`] rather than reaching for
    /// `unwrap_or(..)` on an `Option<HealthCheckConfig>` at a call site.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Active end-to-end probes of the workspace's own artifacts, run on their
    /// own slower cadence inside the eval pass.
    ///
    /// Absent → [`SmokeTestConfig::default`]: the smoke test still runs, but only
    /// the zero-scan `connections` probe, so opting into health checks at all
    /// buys free credential-expiry detection. To turn it off entirely, write
    /// `smoke_test: { enabled: false }`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smoke_test: Option<SmokeTestConfig>,
}

/// Which probes the workspace smoke test runs, and how often.
///
/// **Only `connections` defaults on**, and the whole block defaults on when
/// absent — so a workspace that opted into `health_check:` gets a cheap
/// credential check for free (nothing here runs for one that didn't: the smoke
/// run is gated inside an eval pass). The other three are opt-in even then,
/// because they are not cheap, and the reason is not obvious from their
/// descriptions:
///
/// * `connections` — `SELECT 1`. Scans zero bytes, so it is free on a
///   bytes-billed warehouse (BigQuery on-demand, Athena). Caveat: on Snowflake
///   it *resumes* a suspended warehouse, which bills a 60-second minimum. At the
///   6h default that is four wake-ups a day.
/// * `semantic` — one measure query per topic. `SUM(x)` over an unpartitioned
///   fact table reads the whole column; a `LIMIT` cannot shrink an aggregate's
///   scan. Cost scales with your *data volume*, not your topic count, and on a
///   bytes-billed warehouse it is by far the most expensive probe here.
/// * `agent` — a real LLM round-trip, plus whatever SQL the agent decides to run.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SmokeTestConfig {
    /// When false, no probes run and the dimension reads Healthy.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Smoke cadence as a humantime duration (e.g. `"6h"`, `"1d"`). Absent → 6h.
    /// Clamped to `[10m, 7d]`; unparseable falls back to the default. Because the
    /// smoke run is gated inside an eval pass, a cadence below the `health_check`
    /// interval simply means "every pass".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval: Option<String>,
    /// `SELECT 1` against every database in `databases:`. Defaults **true** — the
    /// one probe cheap enough to run unasked.
    #[serde(default = "default_enabled")]
    pub connections: bool,
    /// One trivial measure query per `.topic.yml`, end to end through the
    /// semantic model to the warehouse. Defaults **false**: it reads the full
    /// measure column on every run, so enabling it against a large unpartitioned
    /// fact table is a real recurring warehouse bill.
    ///
    /// `semantic: true` sweeps every topic with an auto-picked measure; a
    /// `{ topics: [...] }` block probes only what it names. See
    /// [`SemanticProbeConfig`].
    #[serde(default)]
    pub semantic: SemanticProbeConfig,
    /// Ignored. Data Apps (`.app.yml`) were removed and this probe went with
    /// them. Still accepted, because `health_check` rejects unknown keys and a
    /// `config.yml` that sets `apps:` must keep loading.
    ///
    /// Compared as always equal, so a stored smoke config (which no longer records
    /// it) never reads as "changed" against a parsed one that still sets it.
    #[serde(default, skip_serializing)]
    #[schemars(with = "serde_json::Value")]
    pub apps: IgnoredKey,
    /// Ask an agentic agent a fixed question and assert it answers. Absent →
    /// the probe is skipped. Costs tokens on every smoke run.
    ///
    /// Singular back-compat spelling for a single probe; prefer `agents:`. Both
    /// may be set, and [`SmokeTestConfig::resolved_agents`] runs the union.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<SmokeAgentProbe>,
    /// Ask several agents, or the same agent several questions. Each entry is a
    /// separate probe with its own verdict; every one costs tokens on every
    /// smoke run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<SmokeAgentProbe>,
    /// Per-probe-kind cap on how many targets to exercise. Absent → 25.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_targets: Option<usize>,
}

/// A config key that is accepted and thrown away. Every value is equal to every
/// other, and nothing is written back out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IgnoredKey;

impl<'de> Deserialize<'de> for IgnoredKey {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        serde::de::IgnoredAny::deserialize(deserializer)?;
        Ok(IgnoredKey)
    }
}

impl Serialize for IgnoredKey {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_none()
    }
}

/// How the semantic probe picks its targets: sweep everything, or probe a list
/// of named topics.
///
/// ```yaml
/// semantic: true            # every topic, measure auto-picked from its base view
/// semantic:                 # only these topics
///   - topic: fruit_business
///     measures:             # omit to auto-pick one, as the sweep does
///       - fruits.total_count
///       - fruits.avg_price
///   - topic: orders
/// ```
///
/// Naming topics is the cheaper mode and the one to reach for on a real
/// warehouse: the sweep's cost scales with *every* topic's data volume, while a
/// named list is a bill you chose. Explicit `measures` also pin the probe to
/// columns you know are small, instead of whatever the base view lists first.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema)]
#[serde(untagged)]
pub enum SemanticProbeConfig {
    /// `semantic: true|false` — sweep every topic, or run nothing.
    Sweep(bool),
    /// `semantic: [ { topic: … }, … ]` — probe only the named topics.
    Selected(Vec<SemanticProbeTarget>),
}

/// One named topic to probe, and which of its measures to probe it with.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SemanticProbeTarget {
    /// The topic's `name`, as declared in its `.topic.yml`.
    pub topic: String,
    /// Fully-qualified `view.measure` references to query. Empty → the same
    /// auto-pick the sweep uses (first measure on the base view).
    ///
    /// All of a topic's measures go into **one** query rather than one query
    /// each: it is a single warehouse round-trip instead of N, and it exercises
    /// the topic's join graph and fan-out protection, which per-measure queries
    /// never would. The trade is attribution — the verdict names the topic, and
    /// the failing measure shows up in the error text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub measures: Vec<String>,
}

/// The agent round-trip probe: which agent to ask, and what to ask it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SmokeAgentProbe {
    /// Workspace-relative path to the agent config, e.g.
    /// `agents/analytics.agentic.yml`.
    pub agent_ref: String,
    /// The question to ask. Any non-error answer passes — the probe checks that
    /// the pipeline, the LLM key, and SQL generation all work, not that the
    /// answer is correct.
    pub prompt: String,
    /// Label for this probe's verdict. Absent → the `agent_ref`.
    ///
    /// Worth setting once you ask one agent several questions: the verdict's
    /// check id is `agent:<label>`, so same-`agent_ref` probes would otherwise be
    /// told apart only by the `#2` suffix `resolved_agents` appends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// An agent probe with its verdict label resolved and de-duplicated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAgentProbe {
    /// Unique within one smoke run — this is the verdict's `target`.
    pub label: String,
    pub agent_ref: String,
    pub prompt: String,
}

fn default_enabled() -> bool {
    true
}

impl Default for HealthCheckConfig {
    fn default() -> Self {
        Self {
            interval: None,
            enabled: true,
            smoke_test: None,
        }
    }
}

impl Default for SmokeTestConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval: None,
            // Only the zero-scan probe. This Default is what a workspace with no
            // `smoke_test:` block gets, so it must never cost warehouse money
            // nobody asked to spend.
            connections: true,
            semantic: SemanticProbeConfig::Sweep(false),
            apps: IgnoredKey,
            agent: None,
            agents: Vec::new(),
            max_targets: None,
        }
    }
}

impl Default for SemanticProbeConfig {
    fn default() -> Self {
        Self::Sweep(false)
    }
}

impl SemanticProbeConfig {
    /// Whether the probe runs at all. A selection naming no topics is off, not
    /// an enabled probe that checks nothing.
    pub fn enabled(&self) -> bool {
        match self {
            Self::Sweep(on) => *on,
            Self::Selected(targets) => !targets.is_empty(),
        }
    }

    /// The named topics, or `None` when the config asks for a full sweep.
    pub fn selection(&self) -> Option<&[SemanticProbeTarget]> {
        match self {
            Self::Sweep(_) => None,
            Self::Selected(targets) => Some(targets),
        }
    }
}

impl SmokeAgentProbe {
    /// The probe's verdict label before de-duplication.
    fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.agent_ref)
    }
}

impl SmokeTestConfig {
    /// Per-probe-kind target cap, clamped to at least 1 — a `max_targets: 0`
    /// would silently disable the probe while still reporting it as enabled.
    pub fn max_targets(&self) -> usize {
        self.max_targets.unwrap_or(DEFAULT_MAX_TARGETS).max(1)
    }

    /// True when the block is enabled but every individual probe is off, i.e.
    /// there is nothing to run.
    pub fn is_inert(&self) -> bool {
        !self.connections && !self.semantic.enabled() && self.resolved_agents().is_empty()
    }

    /// Every agent probe to run, singular `agent:` first, each with a label that
    /// is unique within the run.
    ///
    /// The de-duplication is load-bearing, not cosmetic: a verdict's check id is
    /// `agent:<label>`, and asking one agent two questions is the obvious reason
    /// to use `agents:` at all. Two verdicts sharing a check id would collide in
    /// the payload's checks table, so a repeated label gets a `#2` suffix.
    pub fn resolved_agents(&self) -> Vec<ResolvedAgentProbe> {
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        self.agent
            .iter()
            .chain(self.agents.iter())
            .map(|probe| {
                let base = probe.label().to_string();
                let count = seen.entry(base.clone()).or_insert(0);
                *count += 1;
                let label = if *count == 1 {
                    base
                } else {
                    format!("{base} #{count}")
                };
                ResolvedAgentProbe {
                    label,
                    agent_ref: probe.agent_ref.clone(),
                    prompt: probe.prompt.clone(),
                }
            })
            .collect()
    }
}

/// Whether a workspace's health eval runs at all.
///
/// **Absent block → disabled.** Health checks are opt-in per workspace: an eval
/// pass costs a warehouse round-trip per enabled probe on every cadence tick, so
/// a workspace that never mentioned `health_check:` must not be spending that.
/// Writing the block is the opt-in; `enabled: false` inside one is the way back
/// out.
///
/// ```yaml
/// # (no health_check block)   → off
/// health_check:               → on, 1h
///   interval: 30m             → on, 30m
/// health_check:
///   enabled: false            → off
/// ```
///
/// This is the single seam every caller must go through, so the "no block"
/// answer can't drift between the compile worker, onboarding, and startup
/// reconcile.
pub fn resolve_enabled(cfg: Option<&HealthCheckConfig>) -> bool {
    cfg.is_some_and(|c| c.enabled)
}

/// Resolve the configured cadence to a clamped `Duration` in `[10m, 24h]`.
/// `None`, an absent `interval`, an unparseable value, or an out-of-range value
/// all resolve to a safe in-range duration (default 1h, or the nearest bound).
pub fn resolve_interval(cfg: Option<&HealthCheckConfig>) -> Duration {
    let secs = cfg
        .and_then(|c| c.interval.as_deref())
        .and_then(|s| humantime::parse_duration(s).ok())
        .map(|d| d.as_secs())
        .unwrap_or(DEFAULT_INTERVAL_SECS)
        .clamp(MIN_INTERVAL_SECS, MAX_INTERVAL_SECS);
    Duration::from_secs(secs)
}

/// Resolve the smoke-test cadence to a clamped `Duration` in `[10m, 7d]`.
/// Same fail-safe contract as [`resolve_interval`]: `None`, an absent
/// `interval`, an unparseable value, or an out-of-range value all resolve to a
/// safe in-range duration (default 6h, or the nearest bound). A malformed
/// cadence must never wedge the eval pass.
pub fn resolve_smoke_interval(cfg: Option<&SmokeTestConfig>) -> Duration {
    let secs = cfg
        .and_then(|c| c.interval.as_deref())
        .and_then(|s| humantime::parse_duration(s).ok())
        .map(|d| d.as_secs())
        .unwrap_or(DEFAULT_SMOKE_INTERVAL_SECS)
        .clamp(MIN_SMOKE_INTERVAL_SECS, MAX_SMOKE_INTERVAL_SECS);
    Duration::from_secs(secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(interval: Option<&str>) -> HealthCheckConfig {
        HealthCheckConfig {
            interval: interval.map(String::from),
            enabled: true,
            smoke_test: None,
        }
    }

    fn smoke(interval: Option<&str>) -> SmokeTestConfig {
        SmokeTestConfig {
            interval: interval.map(String::from),
            ..SmokeTestConfig::default()
        }
    }

    #[test]
    fn absent_config_defaults_to_1h() {
        assert_eq!(resolve_interval(None), Duration::from_secs(3600));
    }

    #[test]
    fn absent_interval_defaults_to_1h() {
        assert_eq!(
            resolve_interval(Some(&cfg(None))),
            Duration::from_secs(3600)
        );
    }

    #[test]
    fn absent_block_is_disabled() {
        // The default that matters: a workspace that never wrote `health_check:`
        // runs no eval, so it never pays for probes it didn't ask for.
        assert!(!resolve_enabled(None));
    }

    #[test]
    fn a_written_block_is_the_opt_in() {
        // Writing the block at all turns it on — `enabled:` need not be spelled
        // out, or `health_check: { interval: 30m }` would be silently inert.
        let hc: HealthCheckConfig = serde_yaml::from_str("{}").unwrap();
        assert!(resolve_enabled(Some(&hc)));
        let hc: HealthCheckConfig = serde_yaml::from_str("interval: 30m").unwrap();
        assert!(resolve_enabled(Some(&hc)));
    }

    #[test]
    fn an_explicit_disable_inside_a_block_still_wins() {
        let hc: HealthCheckConfig = serde_yaml::from_str("enabled: false\ninterval: 30m").unwrap();
        assert!(!resolve_enabled(Some(&hc)));
    }

    #[test]
    fn parses_humantime() {
        assert_eq!(
            resolve_interval(Some(&cfg(Some("30m")))),
            Duration::from_secs(1800)
        );
        assert_eq!(
            resolve_interval(Some(&cfg(Some("2h")))),
            Duration::from_secs(7200)
        );
    }

    #[test]
    fn clamps_below_floor_to_10m() {
        assert_eq!(
            resolve_interval(Some(&cfg(Some("1m")))),
            Duration::from_secs(600)
        );
    }

    #[test]
    fn clamps_above_ceiling_to_24h() {
        assert_eq!(
            resolve_interval(Some(&cfg(Some("72h")))),
            Duration::from_secs(86_400)
        );
    }

    #[test]
    fn unparseable_falls_back_to_1h() {
        assert_eq!(
            resolve_interval(Some(&cfg(Some("banana")))),
            Duration::from_secs(3600)
        );
    }

    #[test]
    fn absent_smoke_config_defaults_to_6h() {
        assert_eq!(resolve_smoke_interval(None), Duration::from_secs(21_600));
        assert_eq!(
            resolve_smoke_interval(Some(&smoke(None))),
            Duration::from_secs(21_600)
        );
    }

    #[test]
    fn smoke_interval_parses_and_clamps() {
        assert_eq!(
            resolve_smoke_interval(Some(&smoke(Some("1d")))),
            Duration::from_secs(86_400)
        );
        // Below the eval floor — a smoke run can't fire more often than the pass
        // that gates it.
        assert_eq!(
            resolve_smoke_interval(Some(&smoke(Some("30s")))),
            Duration::from_secs(600)
        );
        // Above the 7d ceiling.
        assert_eq!(
            resolve_smoke_interval(Some(&smoke(Some("30d")))),
            Duration::from_secs(604_800)
        );
        assert_eq!(
            resolve_smoke_interval(Some(&smoke(Some("banana")))),
            Duration::from_secs(21_600)
        );
    }

    #[test]
    fn smoke_defaults_run_the_zero_scan_probe_only() {
        // This Default is what a workspace with no `smoke_test:` block gets, so
        // every probe that costs warehouse money or tokens must be off.
        let s = SmokeTestConfig::default();
        assert!(s.enabled);
        assert!(
            s.connections,
            "SELECT 1 scans zero bytes — safe to default on"
        );
        assert!(
            !s.semantic.enabled(),
            "a measure query reads the full column — must be opt-in"
        );
        assert!(
            s.resolved_agents().is_empty(),
            "agent probe costs tokens — default off"
        );
        assert!(!s.is_inert());
    }

    #[test]
    fn an_empty_smoke_block_opts_into_nothing_expensive() {
        // `smoke_test: {}` must behave exactly like an absent block: connections
        // only. Turning on the costly probes has to be written down explicitly.
        let s: SmokeTestConfig = serde_yaml::from_str("{}").unwrap();
        assert_eq!(s, SmokeTestConfig::default());
        assert!(s.connections);
        assert!(!s.semantic.enabled());
    }

    #[test]
    fn zero_max_targets_clamps_to_one() {
        // `max_targets: 0` would silently disable a probe that still reports as
        // enabled — clamp rather than honour it.
        let s = SmokeTestConfig {
            max_targets: Some(0),
            ..SmokeTestConfig::default()
        };
        assert_eq!(s.max_targets(), 1);
        assert_eq!(SmokeTestConfig::default().max_targets(), 25);
    }

    #[test]
    fn all_probes_off_is_inert() {
        let s = SmokeTestConfig {
            connections: false,
            semantic: SemanticProbeConfig::Sweep(false),
            agent: None,
            ..SmokeTestConfig::default()
        };
        assert!(s.is_inert());
    }

    #[test]
    fn a_selection_naming_nothing_is_off_not_an_empty_pass() {
        // `semantic: []` must not read as an enabled probe: it would report
        // Healthy having checked nothing, which is a false OK.
        let s = SmokeTestConfig {
            connections: false,
            semantic: serde_yaml::from_str("[]").unwrap(),
            ..SmokeTestConfig::default()
        };
        assert!(!s.semantic.enabled());
        assert!(s.is_inert());
    }

    #[test]
    fn smoke_test_block_deserialises_under_health_check() {
        let yaml = r#"
interval: 10m
smoke_test:
  enabled: true
  interval: 6h
  apps: true
  agent:
    agent_ref: agents/analytics.agentic.yml
    prompt: How many orders were there last week?
"#;
        let hc: HealthCheckConfig = serde_yaml::from_str(yaml).unwrap();
        let s = hc.smoke_test.expect("smoke_test block parsed");
        assert_eq!(
            resolve_smoke_interval(Some(&s)),
            Duration::from_secs(21_600)
        );
        // `apps:` is ignored but must keep parsing: `health_check` rejects unknown keys.
        assert_eq!(s.apps, IgnoredKey);
        // Unspecified probe toggles keep their defaults: cheap on, costly off.
        assert!(s.connections);
        assert!(!s.semantic.enabled());
        let agent = s.agent.expect("agent probe parsed");
        assert_eq!(agent.agent_ref, "agents/analytics.agentic.yml");
    }

    #[test]
    fn semantic_accepts_both_the_sweep_and_a_named_selection() {
        let sweep: SemanticProbeConfig = serde_yaml::from_str("true").unwrap();
        assert_eq!(sweep, SemanticProbeConfig::Sweep(true));
        assert!(sweep.enabled());
        assert!(
            sweep.selection().is_none(),
            "a sweep names no topics — the runner enumerates them"
        );

        let selected: SemanticProbeConfig = serde_yaml::from_str(
            r#"
- topic: fruit_business
  measures:
    - fruits.total_count
    - fruits.avg_price
- topic: orders
"#,
        )
        .unwrap();
        assert!(selected.enabled());
        let topics = selected.selection().expect("named topics");
        assert_eq!(topics.len(), 2);
        assert_eq!(topics[0].topic, "fruit_business");
        assert_eq!(
            topics[0].measures,
            ["fruits.total_count", "fruits.avg_price"],
            "a topic can name several measures"
        );
        // Omitted measures fall back to the same auto-pick the sweep uses.
        assert!(topics[1].measures.is_empty());
    }

    #[test]
    fn multiple_agent_probes_parse_and_keep_their_order() {
        let s: SmokeTestConfig = serde_yaml::from_str(
            r#"
agents:
  - agent_ref: agents/analytics.agentic.yml
    prompt: How many fruits are in the catalog?
  - agent_ref: agents/support.agentic.yml
    prompt: How many open tickets?
    name: support tickets
"#,
        )
        .unwrap();
        let resolved = s.resolved_agents();
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].label, "agents/analytics.agentic.yml");
        assert_eq!(resolved[0].prompt, "How many fruits are in the catalog?");
        // An explicit `name` wins over the agent_ref as the verdict label.
        assert_eq!(resolved[1].label, "support tickets");
        assert!(!s.is_inert());
    }

    #[test]
    fn the_singular_agent_key_still_works_and_composes_with_agents() {
        // `agent:` shipped first. It must keep working, and setting both must run
        // the union rather than silently dropping one.
        let s: SmokeTestConfig = serde_yaml::from_str(
            r#"
agent:
  agent_ref: agents/legacy.agentic.yml
  prompt: still here?
agents:
  - agent_ref: agents/new.agentic.yml
    prompt: and me?
"#,
        )
        .unwrap();
        let resolved = s.resolved_agents();
        assert_eq!(resolved.len(), 2);
        assert_eq!(
            resolved[0].agent_ref, "agents/legacy.agentic.yml",
            "the singular probe runs first"
        );
        assert_eq!(resolved[1].agent_ref, "agents/new.agentic.yml");
    }

    #[test]
    fn asking_one_agent_several_questions_yields_distinct_check_labels() {
        // The whole point of `agents:` is often one agent, several prompts. The
        // verdict check id is `agent:<label>`, so unnamed repeats must not
        // collide into a single row in the checks table.
        let s: SmokeTestConfig = serde_yaml::from_str(
            r#"
agents:
  - agent_ref: a.agentic.yml
    prompt: first?
  - agent_ref: a.agentic.yml
    prompt: second?
  - agent_ref: a.agentic.yml
    prompt: third?
"#,
        )
        .unwrap();
        let labels: Vec<String> = s.resolved_agents().into_iter().map(|a| a.label).collect();
        assert_eq!(
            labels,
            ["a.agentic.yml", "a.agentic.yml #2", "a.agentic.yml #3"]
        );
        let unique: std::collections::BTreeSet<_> = labels.iter().collect();
        assert_eq!(unique.len(), 3, "labels must be unique within a run");
    }

    #[test]
    fn shipped_example_config_parses_with_all_probes_on() {
        // examples/config.yml documents the full smoke surface. Deserialize it
        // through the real `Config` type (which has `deny_unknown_fields`) so a
        // typo'd key or a probe silently switched off can't ship in the example.
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/config.yml");
        let yaml = std::fs::read_to_string(path).expect("read examples/config.yml");
        let cfg: crate::config::model::Config =
            serde_yaml::from_str(&yaml).expect("examples/config.yml must deserialize");
        let smoke = cfg
            .health_check
            .and_then(|h| h.smoke_test)
            .expect("example declares health_check.smoke_test");
        assert!(smoke.enabled, "example smoke test is enabled");
        assert!(smoke.connections && smoke.semantic.enabled());
        let agents = smoke.resolved_agents();
        assert!(!agents.is_empty(), "example enables the agent probe");
        // Every referenced agent file must actually exist in the example project.
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/");
        for agent in &agents {
            assert!(
                std::path::Path::new(root).join(&agent.agent_ref).exists(),
                "agent_ref '{}' must point at a real file in examples/",
                agent.agent_ref
            );
        }
    }
}
