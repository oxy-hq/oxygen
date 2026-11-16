//! What the staff console's resource meters are allowed to assume.
//!
//! Two numbers, and the reason they live in boot config rather than at the
//! read site is the same reason everything else here does: a default and its
//! validation belong in one place, parsed once, visible in
//! `internal-docs/configuration.md`.
//!
//! Both are **deliberately permissive**. A deployment that sets neither still
//! serves the console; the meters simply report less, and say why. Nothing
//! here can refuse a boot, because a cost readout is not worth a restart loop.

/// Settings the Admin → Observability resource meters read.
///
/// Both values are integers, not floats, so this is `Eq` like the rest of
/// `BootConfig` — and money held as cents cannot acquire a rounding error on
/// its way through a comparison. The conversion to `f64` happens where the
/// arithmetic does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeteringConfig {
    /// `OXY_FLEET_COMPUTE_USD_MONTH`: the serving fleet's monthly cost.
    ///
    /// **Unset by default, on purpose.** With no value the compute meter
    /// reports GB-seconds and no dollars. The alternative — defaulting to a
    /// Lambda-style per-GB-second rate — is wrong in both directions at once:
    /// our functions run on pods we pay for whether or not they are busy, so a
    /// marginal serverless rate overstates a busy org and understates what the
    /// idle fleet actually costs.
    ///
    /// A non-positive or unparseable value is `None`, not an error: the meter
    /// then behaves exactly as it does when nothing was configured.
    ///
    /// Held in **cents**: a monthly bill is money, and money in a float is a
    /// rounding error waiting for a comparison.
    pub fleet_compute_cents_month: Option<u64>,

    /// `OXY_FUNCTION_HEAP_LIMIT_MB`: the per-isolate heap ceiling, in MiB.
    ///
    /// Mirrors what the isolate runtime enforces (`oxy-api-custom-apps`'s
    /// `isolate_limits`), because a GB-second figure is wall time multiplied
    /// by exactly this. `0` disables the ceiling there, and here means there
    /// is **no provisioned basis** to convert wall time against — so the meter
    /// reports no quantity rather than dressing bare seconds as GB-seconds.
    ///
    /// The default is 128 MiB, which is Cloudflare Workers' published
    /// per-isolate limit; matching a number an app author can look up is
    /// itself the argument for it.
    pub function_heap_limit_mb: Option<u64>,
}

/// The isolate runtime's default ceiling, in MiB.
const DEFAULT_HEAP_LIMIT_MB: u64 = 128;

impl MeteringConfig {
    pub fn parse(var: &dyn Fn(&str) -> Option<String>) -> Self {
        Self {
            fleet_compute_cents_month: var("OXY_FLEET_COMPUTE_USD_MONTH")
                .and_then(|v| v.trim().parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v > 0.0)
                .map(|usd| (usd * 100.0).round() as u64),
            function_heap_limit_mb: match var("OXY_FUNCTION_HEAP_LIMIT_MB")
                .and_then(|v| v.trim().parse::<i64>().ok())
            {
                // `0` disables the ceiling, which is a configured absence, not
                // a fallback to the default.
                Some(mb) if mb <= 0 => None,
                Some(mb) => Some(mb as u64),
                None => Some(DEFAULT_HEAP_LIMIT_MB),
            },
        }
    }
}

impl MeteringConfig {
    /// The fleet's monthly bill in dollars, for the arithmetic that apportions
    /// it. Cents are the stored form; dollars are the computed one.
    pub fn fleet_compute_usd_month(&self) -> Option<f64> {
        self.fleet_compute_cents_month.map(|c| c as f64 / 100.0)
    }

    /// The provisioned ceiling in MiB, as the GB-second conversion needs it.
    pub fn heap_limit_mb_f64(&self) -> Option<f64> {
        self.function_heap_limit_mb.map(|mb| mb as f64)
    }
}

impl Default for MeteringConfig {
    fn default() -> Self {
        Self::parse(&|_| None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn parse(pairs: &[(&str, &str)]) -> MeteringConfig {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        MeteringConfig::parse(&|name| map.get(name).cloned())
    }

    /// Nothing configured is the ordinary case: a default ceiling and no
    /// fleet bill. The console serves either way.
    #[test]
    fn an_unconfigured_deployment_gets_a_ceiling_and_no_bill() {
        let c = parse(&[]);
        assert_eq!(c.function_heap_limit_mb, Some(128));
        assert_eq!(c.fleet_compute_cents_month, None);
    }

    /// Zero disables the ceiling in the runtime, so there is no provisioned
    /// memory to bill against — a configured absence, not a fallback to 128.
    #[test]
    fn a_zero_ceiling_is_an_absence_not_a_default() {
        assert_eq!(
            parse(&[("OXY_FUNCTION_HEAP_LIMIT_MB", "0")]).function_heap_limit_mb,
            None
        );
        assert_eq!(
            parse(&[("OXY_FUNCTION_HEAP_LIMIT_MB", "-8")]).function_heap_limit_mb,
            None
        );
    }

    /// A nonsense value behaves as unset rather than refusing the boot. A cost
    /// readout is not worth a restart loop.
    #[test]
    fn an_unparseable_value_reads_as_unset() {
        let c = parse(&[
            ("OXY_FLEET_COMPUTE_USD_MONTH", "lots"),
            ("OXY_FUNCTION_HEAP_LIMIT_MB", "huge"),
        ]);
        assert_eq!(c.fleet_compute_cents_month, None);
        assert_eq!(c.function_heap_limit_mb, Some(128));
    }

    /// A non-positive bill is no bill. Apportioning a negative cost would hand
    /// an org a credit it did not earn.
    #[test]
    fn a_non_positive_bill_is_no_bill() {
        for v in ["0", "-250"] {
            assert_eq!(
                parse(&[("OXY_FLEET_COMPUTE_USD_MONTH", v)]).fleet_compute_cents_month,
                None
            );
        }
    }

    #[test]
    fn configured_values_are_read() {
        let c = parse(&[
            ("OXY_FLEET_COMPUTE_USD_MONTH", "1250.50"),
            ("OXY_FUNCTION_HEAP_LIMIT_MB", "256"),
        ]);
        // Money round-trips through cents without a float comparison.
        assert_eq!(c.fleet_compute_cents_month, Some(125_050));
        assert_eq!(c.fleet_compute_usd_month(), Some(1250.50));
        assert_eq!(c.function_heap_limit_mb, Some(256));
        assert_eq!(c.heap_limit_mb_f64(), Some(256.0));
    }
}
