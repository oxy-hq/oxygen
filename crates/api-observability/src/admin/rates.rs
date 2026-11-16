//! What a metered quantity costs, and how confident that number is.
//!
//! Two kinds of line, and keeping them apart is the whole point of this module:
//!
//! 1. **A list price.** Storage and egress are billed by AWS per GB, at a rate
//!    anyone can look up. Multiplying our measured GB by that rate gives a
//!    number that is wrong only by the discount we actually get — an
//!    over-estimate with a known direction.
//!
//! 2. **An apportionment input.** The serving fleet is one bill for every
//!    tenant. There is no per-org rate to look up, and inventing one is the
//!    mistake [`internal-docs/2026-11-16-admin-observability-design.md`]
//!    rejected. So this module **never invents the fleet's cost**: an operator
//!    supplies the monthly figure, and the share each org carries is then
//!    arithmetic over a measured driver. With nothing supplied, the meter
//!    reports its quantity and says plainly that no cost was configured.
//!
//! LLM is neither: it is priced per model by `oxy_app_core::llm_usage::price`,
//! from a rate table we already trust, and it is reported on the Cost panel. It
//! is deliberately absent here so the two panels cannot double-count.
//!
//! ## Why list prices live in code and not in a settings table
//!
//! They move about once a year and a wrong one is a one-line fix with a diff
//! anyone can review against a public page. A settings table would make the
//! number invisible, unversioned, and different on every deployment — which is
//! worse for a figure whose whole value is that a reader can check it.

/// AWS S3 Standard, first 50 TB, us-east-1 — USD per GB-month.
///
/// <https://aws.amazon.com/s3/pricing/>. Custom-app storage is S3, so this is
/// the right list price; it ignores request charges, which are noise against
/// the stored bytes at our volumes.
pub const S3_STANDARD_USD_PER_GB_MONTH: f64 = 0.023;

/// AWS data transfer out to the internet, first 10 TB/month — USD per GB.
///
/// <https://aws.amazon.com/ec2/pricing/on-demand/>. An over-estimate for
/// anything served through CloudFront, which is cheaper; named as list price so
/// a reader knows which way it errs.
pub const EGRESS_USD_PER_GB: f64 = 0.09;

/// The env var holding the serving fleet's monthly cost, named so a response
/// can tell an operator which one to set. Parsed in
/// `oxy_app_core::config::MeteringConfig`, not here: a default and its
/// validation belong in one place, and the env-read guard
/// (`crates/app/tests/routing/env_reads`) enforces exactly that.
pub const FLEET_COST_ENV: &str = "OXY_FLEET_COMPUTE_USD_MONTH";

/// The memory a function invocation is provisioned, in MiB.
///
/// From boot config, which mirrors what the isolate runtime enforces. `None`
/// means the ceiling is disabled there, so a GB-second figure has **no
/// provisioned basis** — the compute meter then reports no quantity rather
/// than dressing bare wall-clock seconds as GB-seconds.
pub fn provisioned_memory_mb() -> Option<f64> {
    oxy_app_core::config::BootConfig::current()
        .metering
        .heap_limit_mb_f64()
}

/// The serving fleet's monthly cost, if an operator supplied one.
pub fn fleet_cost_usd_month() -> Option<f64> {
    oxy_app_core::config::BootConfig::current()
        .metering
        .fleet_compute_usd_month()
}

/// Gigabyte-seconds from wall-clock milliseconds at a provisioned ceiling.
///
/// **This is provisioned, not consumed.** It is Vercel's legacy GB-hours model,
/// and `internal-docs/platform-metrics.md` names its flaw where it lives:
/// *"wall clock punishes an app whose warehouse query is slow while missing a
/// tight loop that finishes inside the budget"*. An invocation parked on a host
/// call bills the same as one spinning. Cloudflare moved to Active CPU for this
/// reason, and we cannot follow until there is CPU metering — which the same
/// doc lists as not done.
///
/// `None` when no ceiling is in force: without one there is no memory figure to
/// multiply by, and a bare second is not a GB-second.
pub fn gb_seconds(wall_ms: f64, provisioned_mb: Option<f64>) -> Option<f64> {
    let mb = provisioned_mb?;
    if wall_ms <= 0.0 {
        return Some(0.0);
    }
    Some((wall_ms / 1000.0) * (mb / 1024.0))
}

/// One org's share of a shared bill, by its share of a measured driver.
///
/// `None` when the fleet total is zero — nobody used it, so nobody's share is
/// anything, and 0/0 is not 100%. `None` too when no bill was supplied, which
/// the caller passes through as "quantity only".
pub fn apportion(org_driver: f64, fleet_driver: f64, bill_usd: Option<f64>) -> Option<f64> {
    let bill = bill_usd?;
    if fleet_driver <= 0.0 || !fleet_driver.is_finite() {
        return None;
    }
    Some(bill * (org_driver / fleet_driver))
}

/// Scale a monthly figure to the window being viewed.
///
/// A fleet bill is quoted per month; a 7-day view must not show a month of it.
pub fn month_to_window(usd_month: f64, window_days: u32) -> f64 {
    usd_month * (f64::from(window_days) / 30.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ceiling of zero disables containment in the runtime, so there is no
    /// provisioned memory to bill against. Reporting 128 MiB anyway would
    /// invent the very basis the figure rests on.
    #[test]
    fn a_disabled_ceiling_has_no_provisioned_basis() {
        assert_eq!(gb_seconds(10_000.0, None), None);
    }

    /// 1 second at 1 GiB is 1 GB-second; the 128 MiB default is an eighth of
    /// that. Pinned because every compute figure on the surface is this
    /// multiplication.
    #[test]
    fn gb_seconds_convert_wall_time_at_the_ceiling() {
        assert_eq!(gb_seconds(1_000.0, Some(1024.0)), Some(1.0));
        assert_eq!(gb_seconds(8_000.0, Some(128.0)), Some(1.0));
        assert_eq!(gb_seconds(0.0, Some(128.0)), Some(0.0));
    }

    /// No bill supplied means no cost, not a guessed one. This is the rule the
    /// module exists for: we never invent the fleet's price.
    #[test]
    fn no_supplied_bill_means_no_apportioned_cost() {
        assert_eq!(apportion(50.0, 100.0, None), None);
    }

    /// An idle fleet gives nobody a share. 0/0 is not 100%, and an org that
    /// used nothing on a fleet nobody used must not be handed the whole bill.
    #[test]
    fn an_idle_fleet_apportions_nothing() {
        assert_eq!(apportion(0.0, 0.0, Some(1_000.0)), None);
        assert_eq!(apportion(5.0, 0.0, Some(1_000.0)), None);
    }

    /// A share is the org's driver over the fleet's, times the bill.
    #[test]
    fn a_share_is_the_drivers_ratio_times_the_bill() {
        assert_eq!(apportion(25.0, 100.0, Some(400.0)), Some(100.0));
        assert_eq!(apportion(100.0, 100.0, Some(400.0)), Some(400.0));
    }

    /// A monthly bill scales to the window. A 7-day view showing a month of
    /// fleet cost beside 7 days of LLM spend would make the two incomparable,
    /// which is the one thing a cost page must not do.
    #[test]
    fn a_monthly_bill_scales_to_the_window() {
        assert_eq!(month_to_window(300.0, 30), 300.0);
        assert_eq!(month_to_window(300.0, 7), 70.0);
        assert_eq!(month_to_window(300.0, 90), 900.0);
    }

    /// The list prices are the published ones. Pinned so a change is a
    /// deliberate edit with a diff, not a drift nobody notices — the whole
    /// reason they live in code.
    #[test]
    fn list_prices_are_the_published_aws_rates() {
        assert_eq!(S3_STANDARD_USD_PER_GB_MONTH, 0.023);
        assert_eq!(EGRESS_USD_PER_GB, 0.09);
    }
}
