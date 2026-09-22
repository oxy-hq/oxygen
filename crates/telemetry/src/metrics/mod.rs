//! The third signal: metrics.
//!
//! Oxy exported traces and logs and nothing else, which left route latency,
//! error rate and saturation answerable only by scanning spans in ClickHouse.
//! This module adds the missing signal, with **one instrument set behind two
//! readers**:
//!
//! - a [`ManualReader`] rendered as Prometheus text on the ops port — the path
//!   that works today, because VictoriaMetrics already scrapes this fleet
//!   (`worker_metrics`' series carry `job="oxy-worker"` and drive an HPA);
//! - an OTLP [`PeriodicReader`], **opt-in** via `OTEL_METRICS_EXPORTER=otlp`,
//!   for the day the cluster collector grows a metrics pipeline.
//!
//! The opt-in is not caution, it is the measured state of the collector:
//! `otel-logs-collector` runs `logs: [filelog]`, `metrics: [kubeletstats]`,
//! `traces: [otlp]`. The OTLP receiver is wired to traces only, so metrics
//! pushed over OTLP today land nowhere. Defaulting it on would mean every
//! process retrying an export every 60s against an endpoint that drops it —
//! exactly the reasoning that already makes log export opt-in in [`crate::otel`].
//!
//! ## Why two readers rather than one
//!
//! A single OTLP reader ships dark until an infrastructure change lands in
//! another repository. A single Prometheus path means hand-maintaining
//! histogram bucket accounting (see [`exposition`] for why that is the part
//! worth refusing). Two readers over one instrument set costs one newtype and
//! makes the transport a deployment decision rather than a code change.
//!
//! ## What is deliberately not here
//!
//! `worker_metrics` is untouched. Its 1,030 lines encode absent-vs-zero rules,
//! three-matcher alert recipes and an explicit "`max` must not be used" for
//! `oxy_metrics_scrape_db_ok` — semantics a generic exporter flattens. Its
//! output is concatenated with this module's, byte-for-byte unchanged.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use opentelemetry::metrics::MeterProvider as _;
use opentelemetry_otlp::WithExportConfig as _;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::metrics::data::ResourceMetrics;
use opentelemetry_sdk::metrics::reader::MetricReader;
use opentelemetry_sdk::metrics::{
    InstrumentKind, ManualReader, PeriodicReader, SdkMeterProvider, Temporality,
};

pub mod exposition;
pub mod instruments;
pub mod record;
pub mod sources;

pub use instruments::Instruments;

/// The instrumentation scope every Oxy instrument is created under.
const SCOPE: &str = "oxy";

/// How long a provider shutdown may take before we stop waiting. Matches the
/// trace/log exporters' budget in [`crate::otel`].
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Default OTLP push interval. The spec default is 60s; kept explicit because
/// the Prometheus reader's effective interval is the scrape interval, and
/// having the two visibly differ avoids "why is OTLP behind" confusion.
const DEFAULT_EXPORT_INTERVAL: Duration = Duration::from_secs(60);

/// `OTEL_METRICS_EXPORTER` — `otlp` turns OTLP push on. Unset is off.
const METRICS_EXPORTER_ENV: &str = "OTEL_METRICS_EXPORTER";
/// `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT`, else the generic endpoint.
const METRICS_ENDPOINT_ENV: &str = "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT";
const GENERIC_ENDPOINT_ENV: &str = "OTEL_EXPORTER_OTLP_ENDPOINT";
const SDK_DISABLED_ENV: &str = "OTEL_SDK_DISABLED";
/// `OTEL_METRIC_EXPORT_INTERVAL`, in milliseconds, per the SDK spec.
const EXPORT_INTERVAL_ENV: &str = "OTEL_METRIC_EXPORT_INTERVAL";

/// A [`ManualReader`] the meter provider and the `/metrics` handler can both
/// hold.
///
/// `MeterProviderBuilder::with_reader` takes the reader by value and
/// `ManualReader` is neither `Clone` nor covered by a blanket `MetricReader`
/// impl for `Arc`, so a scrape handler has no way to reach the reader the
/// provider swallowed. This newtype is the whole fix: `Arc` inside, delegation
/// out.
#[derive(Debug, Clone)]
pub struct SharedManualReader(Arc<ManualReader>);

impl SharedManualReader {
    pub(crate) fn new() -> Self {
        // Cumulative is what Prometheus needs — a counter that resets between
        // scrapes would make `rate()` meaningless.
        Self(Arc::new(
            ManualReader::builder()
                .with_temporality(Temporality::Cumulative)
                .build(),
        ))
    }
}

impl MetricReader for SharedManualReader {
    fn register_pipeline(&self, pipeline: std::sync::Weak<opentelemetry_sdk::metrics::Pipeline>) {
        self.0.register_pipeline(pipeline);
    }

    fn collect(&self, rm: &mut ResourceMetrics) -> OTelSdkResult {
        self.0.collect(rm)
    }

    fn force_flush(&self) -> OTelSdkResult {
        self.0.force_flush()
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.0.shutdown_with_timeout(timeout)
    }

    fn temporality(&self, kind: InstrumentKind) -> Temporality {
        self.0.temporality(kind)
    }
}

/// What the environment says about metrics export.
#[derive(Debug, Clone)]
pub struct MetricsConfig {
    /// `OTEL_SDK_DISABLED=true` — no provider at all, instruments become no-ops.
    pub sdk_disabled: bool,
    /// `OTEL_METRICS_EXPORTER=otlp`. Opt-in; see the module doc for why.
    pub otlp_enabled: bool,
    /// Where OTLP metrics go, if anywhere.
    pub otlp_endpoint: Option<String>,
    /// OTLP push interval.
    pub export_interval: Duration,
}

impl MetricsConfig {
    /// Read the standard OpenTelemetry environment contract.
    pub fn from_env() -> Self {
        Self {
            sdk_disabled: env(SDK_DISABLED_ENV)
                .map(|v| v.eq_ignore_ascii_case("true"))
                .unwrap_or(false),
            otlp_enabled: metrics_exporter_enabled(env(METRICS_EXPORTER_ENV).as_deref()),
            otlp_endpoint: env(METRICS_ENDPOINT_ENV).or_else(|| env(GENERIC_ENDPOINT_ENV)),
            export_interval: env(EXPORT_INTERVAL_ENV)
                .and_then(|v| v.parse::<u64>().ok())
                .map(Duration::from_millis)
                .unwrap_or(DEFAULT_EXPORT_INTERVAL),
        }
    }

    /// Whether the OTLP reader should be attached.
    pub fn otlp_exported(&self) -> bool {
        !self.sdk_disabled && self.otlp_enabled && self.otlp_endpoint.is_some()
    }
}

/// `OTEL_METRICS_EXPORTER` semantics: on only when it says `otlp`.
///
/// Deliberately **not** the spec's "unset means otlp" default, which
/// [`crate::otel`] also declines for logs. The reason is the same and it is
/// specific to this cluster: the collector's metrics pipeline has no OTLP
/// receiver, so honouring the default would have every process exporting into
/// a void on a 60s timer.
pub fn metrics_exporter_enabled(value: Option<&str>) -> bool {
    matches!(value, Some(v) if v.eq_ignore_ascii_case("otlp"))
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

/// Everything the process needs to serve and ship metrics.
struct Installed {
    provider: SdkMeterProvider,
    reader: SharedManualReader,
    instruments: Instruments,
}

/// A plain `OnceLock`, **not** a `Mutex<Option<_>>`, and the difference is on
/// the hot path.
///
/// [`with_instruments`] runs twice per HTTP request. Behind a mutex that is two
/// lock acquisitions per request on a shared static — uncontended it is cheap,
/// but it is a process-wide serialization point that gets worse exactly as
/// concurrency rises, which is when the metrics matter most. A `OnceLock` read
/// is an atomic load.
///
/// What that costs is the ability to *remove* the installation: `OnceLock` has
/// no stable take. Nothing needs to — installation happens once at boot and
/// `SdkMeterProvider::shutdown_with_timeout` takes `&self`, so shutdown flushes
/// in place and the provider is dropped when the process exits.
static INSTALLED: OnceLock<Installed> = OnceLock::new();

/// Build the meter provider, register the instruments, and install both
/// globally. Returns human-readable problems the caller logs alongside the
/// other telemetry startup diagnostics.
///
/// Idempotent by refusal: a second call leaves the first installation in place
/// and says so, matching [`crate::otel::install`].
pub fn init(config: &MetricsConfig, resource: Resource) -> Vec<String> {
    let mut problems = Vec::new();
    if config.sdk_disabled {
        return problems;
    }
    if INSTALLED.get().is_some() {
        problems.push("metrics were already installed; the earlier provider is what serves".into());
        return problems;
    }

    let reader = SharedManualReader::new();
    let mut builder = SdkMeterProvider::builder()
        .with_resource(resource)
        .with_reader(reader.clone());

    if config.otlp_exported() {
        match build_otlp_reader(config) {
            Ok(periodic) => builder = builder.with_reader(periodic),
            Err(err) => problems.push(format!("OTLP metric exporter disabled: {err}")),
        }
    }

    let provider = builder.build();
    let instruments = Instruments::new(&provider.meter(SCOPE));
    opentelemetry::global::set_meter_provider(provider.clone());

    if INSTALLED
        .set(Installed {
            provider,
            reader,
            instruments,
        })
        .is_err()
    {
        // Lost a race with a concurrent `init`. The winner's provider is the
        // one `global::set_meter_provider` may or may not have kept, so say so
        // rather than pretend this call installed anything.
        problems
            .push("metrics were installed concurrently; one of the two providers serves".into());
    }
    problems
}

fn build_otlp_reader(
    config: &MetricsConfig,
) -> Result<
    PeriodicReader<opentelemetry_otlp::MetricExporter>,
    opentelemetry_otlp::ExporterBuildError,
> {
    // Endpoint, headers and timeout come from the OTEL_EXPORTER_OTLP_*
    // variables the builder reads itself; only the wire protocol is pinned,
    // because only the HTTP transport is compiled in.
    let exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .with_protocol(opentelemetry_otlp::Protocol::HttpBinary)
        .build()?;
    Ok(PeriodicReader::builder(exporter)
        .with_interval(config.export_interval)
        .build())
}

/// Run `f` against the installed instrument set, or do nothing.
///
/// Every recording site goes through this, twice per HTTP request. It is an
/// atomic load and a branch — see [`INSTALLED`] for why it is not a lock. When
/// no provider is installed (`OTEL_SDK_DISABLED`, a unit test, a CLI
/// subcommand that never serves) the closure is skipped, so no call site needs
/// its own guard.
pub fn with_instruments<F: FnOnce(&Instruments)>(f: F) {
    if let Some(installed) = INSTALLED.get() {
        f(&installed.instruments);
    }
}

/// Render the current values as Prometheus exposition text.
///
/// Empty when no provider is installed, so a caller can concatenate
/// unconditionally.
pub fn render_prometheus() -> String {
    match INSTALLED.get() {
        Some(installed) => exposition::render(&installed.reader),
        None => String::new(),
    }
}

/// Whether a provider is installed — the honest answer to "is `/metrics` going
/// to have anything in it", which a handler needs in order to distinguish "no
/// metrics configured" from "no traffic yet".
pub fn is_installed() -> bool {
    INSTALLED.get().is_some()
}

/// Flush the meter provider. Called from the same shutdown path as
/// [`crate::otel::shutdown`].
///
/// Flushes in place rather than removing the installation — `OnceLock` has no
/// stable take, and nothing needs one: this runs once, immediately before the
/// process exits. A recording that races it is dropped by the provider itself,
/// which is the same guarantee the trace and log exporters give.
pub fn shutdown() -> Vec<String> {
    let mut problems = Vec::new();
    if let Some(installed) = INSTALLED.get()
        && let Err(err) = installed.provider.shutdown_with_timeout(SHUTDOWN_TIMEOUT)
    {
        problems.push(format!("metrics provider shutdown: {err}"));
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn otlp_is_off_unless_explicitly_asked_for() {
        // The spec default is "unset means otlp". We decline it, because the
        // cluster collector's metrics pipeline has no OTLP receiver.
        assert!(!metrics_exporter_enabled(None));
        assert!(!metrics_exporter_enabled(Some("")));
        assert!(!metrics_exporter_enabled(Some("none")));
        assert!(!metrics_exporter_enabled(Some("prometheus")));
        assert!(metrics_exporter_enabled(Some("otlp")));
        assert!(metrics_exporter_enabled(Some("OTLP")));
    }

    #[test]
    fn otlp_needs_both_the_switch_and_an_endpoint() {
        let base = MetricsConfig {
            sdk_disabled: false,
            otlp_enabled: true,
            otlp_endpoint: Some("http://collector:4318".into()),
            export_interval: DEFAULT_EXPORT_INTERVAL,
        };
        assert!(base.otlp_exported());

        assert!(
            !MetricsConfig {
                otlp_endpoint: None,
                ..base.clone()
            }
            .otlp_exported(),
            "an endpoint-less export would retry into nothing every interval"
        );
        assert!(
            !MetricsConfig {
                otlp_enabled: false,
                ..base.clone()
            }
            .otlp_exported()
        );
        assert!(
            !MetricsConfig {
                sdk_disabled: true,
                ..base
            }
            .otlp_exported(),
            "OTEL_SDK_DISABLED is the kill switch for every signal"
        );
    }

    /// The Prometheus reader must not depend on the OTLP switch: the whole
    /// point of two readers is that the scrape path works with the collector
    /// exactly as it is today.
    #[test]
    fn rendering_is_safe_before_anything_is_installed() {
        // Not asserting emptiness — another test in this binary may have
        // installed a provider. Asserting it does not panic is the contract
        // that matters for a handler that calls this unconditionally.
        let _ = render_prometheus();
        with_instruments(|_| {});
    }
}
