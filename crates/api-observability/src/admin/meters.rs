//! The meters themselves: what each resource is, what it costs, and how far
//! the figure can be trusted.
//!
//! Split out of `resources.rs`, which holds the handler and the wire shapes.
//! One builder per meter, so adding a resource is a function rather than
//! another branch in a list that was already past the file-size rule.
//!
//! Every builder returns a [`Meter`] rather than pushing onto a list: a
//! function that can only return one meter cannot accidentally emit two, or
//! none, and each is testable on its own.

use super::resources::{Attribution, Meter, MeterInput};
use super::{rates, storage_rollup};

/// Bytes per GB, decimal — egress is billed in decimal gigabytes, unlike
/// storage, which is binary. Mixing them is a 7% error in the direction of
/// over-billing, which is exactly the kind of quiet wrongness this panel is
/// supposed to be better than.
const BYTES_PER_GB_DECIMAL: f64 = 1_000_000_000.0;

/// The meter list, pure. Ordered by how much a reader can rely on it: direct
/// first, then apportioned, then what we cannot answer.
pub(super) fn build_meters(input: MeterInput<'_>) -> Vec<Meter> {
    let mut meters = Vec::new();
    meters.push(storage_meter(&input));
    meters.push(egress_meter(&input));
    meters.push(compute_meter(&input));
    meters.push(airhouse_meter(&input));
    meters.push(oltp_meter());
    meters
}

/// App storage: measured from the sample series, priced at S3 list.
fn storage_meter(input: &MeterInput<'_>) -> Meter {
    match &input.storage {
        // Apps exist and at least one reported. A measured figure.
        Some(s) if s.apps_with_samples > 0 => {
            let gib_month = s.gib * (f64::from(input.window_days) / 30.0);
            Meter {
                key: "storage",
                label: "App storage",
                quantity: Some(gib_month),
                unit: "GiB-month",
                cost_usd: Some(gib_month * rates::S3_STANDARD_USD_PER_GB_MONTH),
                attribution: Attribution::Direct,
                note: None,
            }
        }
        // Either no storage rows at all, or rows with no readings. Both are
        // "we did not measure", and `metering` is explicit that zero samples
        // must never be billed as zero usage.
        _ => Meter {
            key: "storage",
            label: "App storage",
            quantity: None,
            unit: "GiB-month",
            cost_usd: None,
            attribution: Attribution::Unmeasured,
            note: Some(
                "No storage readings in this window. The sweeper appends a sample only when \
                 its walk completes, so this is missing data rather than an empty bucket."
                    .into(),
            ),
        },
    }
}

/// App egress: measured bytes, priced at AWS list.
fn egress_meter(input: &MeterInput<'_>) -> Meter {
    match input.traffic {
        Some(t) => {
            let gb = t.bytes as f64 / BYTES_PER_GB_DECIMAL;
            Meter {
                key: "egress",
                label: "App egress",
                quantity: Some(gb),
                unit: "GB",
                cost_usd: Some(gb * rates::EGRESS_USD_PER_GB),
                attribution: Attribution::Direct,
                note: Some(
                    "AWS list price for transfer to the internet. Anything served through \
                     CloudFront costs less, so this errs high."
                        .into(),
                ),
            }
        }
        None => unmeasured_app_meter("egress", "App egress", "GB"),
    }
}

/// Function compute: a share of the serving fleet, by measured wall time.
fn compute_meter(input: &MeterInput<'_>) -> Meter {
    match input.traffic {
        Some(t) => {
            // Summed wall time, not `requests × p50`: a median times a count
            // is not the area under the curve, and a long tail makes the two
            // differ by most of the figure.
            let wall_ms = t.wall_ms as f64;
            let gbs = rates::gb_seconds(wall_ms, input.provisioned_mb);
            let cost = input
                .fleet_wall_ms
                .and_then(|fleet| rates::apportion(wall_ms, fleet, input.fleet_cost))
                .map(|m| rates::month_to_window(m, input.window_days));
            Meter {
                key: "function_compute",
                label: "Function compute",
                quantity: gbs,
                unit: "GB·s",
                cost_usd: cost,
                attribution: Attribution::Apportioned,
                note: Some(compute_note(
                    input.provisioned_mb,
                    input.fleet_cost,
                    cost.is_some(),
                )),
            }
        }
        None => unmeasured_app_meter("function_compute", "Function compute", "GB·s"),
    }
}

/// The org's database: directly billable, and not read yet.
fn oltp_meter() -> Meter {
    Meter {
        key: "oltp",
        label: "Org database (OLTP)",
        quantity: None,
        unit: "project-month",
        cost_usd: None,
        attribution: Attribution::Unmeasured,
        note: Some(
            "Each org has its own Neon project, so this is directly billable rather than \
             shared — but nothing here reads Neon's consumption API yet."
                .into(),
        ),
    }
}

/// Airhouse: the provisioned unit is real and countable; what it holds and
/// what it burns are not, yet. Reporting the count with both gaps named beats
/// a blanket "unmeasured", which would read as "this org has none".
fn airhouse_meter(input: &MeterInput<'_>) -> Meter {
    match input.airhouse_tenants {
        Some(n) => Meter {
            key: "warehouse",
            label: "Airhouse",
            quantity: Some(n as f64),
            unit: "tenant",
            cost_usd: None,
            attribution: Attribution::Direct,
            note: Some(
                "Provisioned tenants, one per workspace — countable and directly the \
                 org's. The bytes each holds need an S3 walk like the app-storage \
                 sweeper's, and query time needs metering at the connector; neither \
                 exists, so there is no cost here yet."
                    .into(),
            ),
        },
        None => Meter {
            key: "warehouse",
            label: "Airhouse",
            quantity: None,
            unit: "tenant",
            cost_usd: None,
            attribution: Attribution::Unmeasured,
            note: Some("Could not read the org's Airhouse tenants.".into()),
        },
    }
}

/// What a compute figure rests on, said plainly.
fn compute_note(provisioned_mb: Option<f64>, fleet_cost: Option<f64>, priced: bool) -> String {
    let basis = match provisioned_mb {
        Some(mb) => format!(
            "Provisioned, not consumed: wall time at the {mb:.0} MiB isolate ceiling. An \
             invocation waiting on a host call bills the same as one computing — there is no \
             CPU metering to use instead."
        ),
        None => "The isolate heap ceiling is disabled, so there is no provisioned memory to \
                 convert wall time against."
            .into(),
    };
    if priced {
        return format!("{basis} Cost is this org's share of the serving fleet, by wall time.");
    }
    match fleet_cost {
        Some(_) => format!("{basis} No fleet-wide total on this view, so no share is computed."),
        None => format!(
            "{basis} No cost shown: set {} to the fleet's monthly bill and the share becomes \
             arithmetic.",
            rates::FLEET_COST_ENV
        ),
    }
}

/// A meter whose source is the observability store, when that store is absent.
fn unmeasured_app_meter(key: &'static str, label: &'static str, unit: &'static str) -> Meter {
    Meter {
        key,
        label,
        quantity: None,
        unit,
        cost_usd: None,
        attribution: Attribution::Unmeasured,
        note: Some(
            "This deployment has no observability store, so custom-app traffic is not \
             captured at all."
                .into(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxy_observability::types::OrgAppTotals;

    /// Traffic totals as the compute meter reads them: `wall_ms` is the
    /// figure it uses, so the helper takes it rather than a median that no
    /// longer feeds anything.
    fn totals(bytes: u64, requests: u64, wall_ms: u64) -> OrgAppTotals {
        OrgAppTotals {
            requests,
            bytes,
            wall_ms,
            ..Default::default()
        }
    }

    fn find<'a>(meters: &'a [Meter], key: &str) -> &'a Meter {
        meters.iter().find(|m| m.key == key).expect("meter present")
    }

    fn input<'a>(traffic: Option<&'a OrgAppTotals>) -> MeterInput<'a> {
        MeterInput {
            window_days: 30,
            storage: None,
            traffic,
            provisioned_mb: Some(128.0),
            fleet_cost: None,
            fleet_wall_ms: None,
            airhouse_tenants: None,
        }
    }

    /// Storage with no readings is **unmeasured**, not zero. The meter it
    /// reuses says so in its own words — zero samples "is NOT the same as zero
    /// usage and must never be billed as such" — and a zero on this panel
    /// would be billed as such by any reader.
    #[test]
    fn storage_with_no_readings_is_unmeasured_not_zero() {
        let meters = build_meters(input(None));
        let m = find(&meters, "storage");
        assert_eq!(m.attribution, Attribution::Unmeasured);
        assert_eq!(m.quantity, None);
        assert_eq!(m.cost_usd, None);
        assert!(m.note.as_ref().unwrap().contains("missing data"));
    }

    /// An org with apps that did report meters normally and prices at list.
    #[test]
    fn storage_with_readings_meters_and_prices_at_list() {
        let mut i = input(None);
        i.storage = Some(storage_rollup::OrgStorage {
            average_bytes: 0.0,
            gib: 100.0,
            apps_with_samples: 3,
        });
        let meters = build_meters(i);
        let m = find(&meters, "storage");
        assert_eq!(m.attribution, Attribution::Direct);
        assert_eq!(m.quantity, Some(100.0), "a 30-day window is one month");
        assert_eq!(m.cost_usd, Some(100.0 * 0.023));
    }

    /// A 7-day window bills a quarter of the month's storage, not a month of
    /// it. The storage rate is per GB-*month*, so the window has to be applied
    /// or a weekly view reports four times the cost.
    #[test]
    fn a_short_window_scales_the_storage_month() {
        let mut i = input(None);
        i.window_days = 7;
        i.storage = Some(storage_rollup::OrgStorage {
            average_bytes: 0.0,
            gib: 30.0,
            apps_with_samples: 1,
        });
        let meters = build_meters(i);
        let m = find(&meters, "storage").quantity.unwrap();
        assert!(
            (m - 7.0).abs() < 1e-9,
            "30 GiB for 7/30 of a month, got {m}"
        );
    }

    /// Egress is decimal GB. Storage is binary GiB. Using one for the other is
    /// a silent 7% over-bill, which is precisely the quiet wrongness this
    /// panel is supposed to improve on.
    #[test]
    fn egress_is_decimal_gigabytes_not_binary() {
        let t = totals(1_000_000_000, 0, 0);
        let meters = build_meters(input(Some(&t)));
        let m = find(&meters, "egress");
        assert_eq!(m.quantity, Some(1.0));
        assert_eq!(BYTES_PER_GB_DECIMAL, 1e9);
    }

    /// Compute is always apportioned, never direct — it is a share of pods we
    /// pay for whether or not they are busy.
    #[test]
    fn compute_is_apportioned_never_direct() {
        let t = totals(0, 1_000, 8_000);
        let meters = build_meters(input(Some(&t)));
        let m = find(&meters, "function_compute");
        assert_eq!(m.attribution, Attribution::Apportioned);
    }

    /// With no fleet bill supplied there is a quantity and no cost, and the
    /// note names the env var that would change that. Inventing a per-GB-second
    /// rate here is the one thing the rate module refuses to do.
    #[test]
    fn compute_without_a_fleet_bill_reports_quantity_only() {
        let t = totals(0, 1_000, 8_000);
        let meters = build_meters(input(Some(&t)));
        let m = find(&meters, "function_compute");
        // 8,000 ms of wall time = 8 s at 128 MiB = 1 GB·s.
        assert_eq!(m.quantity, Some(1.0));
        assert_eq!(m.cost_usd, None);
        assert!(m.note.as_ref().unwrap().contains(rates::FLEET_COST_ENV));
    }

    /// Every compute note states that the figure is provisioned rather than
    /// consumed. A GB-second that does not say so reads as measured CPU, which
    /// it is not and cannot be until there is CPU metering.
    #[test]
    fn the_compute_note_always_says_provisioned_not_consumed() {
        let t = totals(0, 10, 5_000);
        let meters = build_meters(input(Some(&t)));
        let m = find(&meters, "function_compute");
        let note = m.note.as_ref().unwrap();
        assert!(note.contains("Provisioned, not consumed"), "{note}");
        assert!(note.contains("no CPU metering"), "{note}");
    }

    /// With a fleet bill and a fleet-wide driver, the share is arithmetic and
    /// scaled to the window.
    #[test]
    fn compute_with_a_fleet_bill_apportions_by_wall_time() {
        let t = totals(0, 1_000, 10_000); // 10,000 ms of wall time
        let mut i = input(Some(&t));
        i.fleet_cost = Some(300.0);
        i.fleet_wall_ms = Some(40_000.0); // this org is a quarter of the fleet
        let meters = build_meters(i);
        let m = find(&meters, "function_compute");
        assert_eq!(m.cost_usd, Some(75.0), "a quarter of a 300/mo fleet");
        assert!(
            m.note
                .as_ref()
                .unwrap()
                .contains("share of the serving fleet")
        );
    }

    /// With no observability store every app-sourced meter is unmeasured and
    /// says why — not a row of zeros implying a quiet org.
    #[test]
    fn no_observability_store_makes_app_meters_unmeasured() {
        let meters = build_meters(input(None));
        for key in ["egress", "function_compute"] {
            let m = find(&meters, key);
            assert_eq!(m.attribution, Attribution::Unmeasured, "{key}");
            assert_eq!(m.quantity, None, "{key}");
            assert!(m.note.as_ref().unwrap().contains("no observability store"));
        }
    }

    /// Airhouse reports the unit that is real — provisioned tenants — and
    /// names the two things that are not. A blanket "unmeasured" would read
    /// as "this org has no warehouse", which is a different and wrong claim.
    #[test]
    fn airhouse_counts_what_is_countable_and_names_what_is_not() {
        let mut i = input(None);
        i.airhouse_tenants = Some(3);
        let meters = build_meters(i);
        let m = find(&meters, "warehouse");
        assert_eq!(m.quantity, Some(3.0));
        assert_eq!(m.attribution, Attribution::Direct);
        assert_eq!(m.cost_usd, None, "a count is not a bill");
        let note = m.note.as_ref().unwrap();
        assert!(note.contains("S3 walk"), "{note}");
        assert!(note.contains("query time"), "{note}");
    }

    /// A failed read is absent, not zero. Zero tenants and "we could not look"
    /// are different answers, and only one of them means the org has no
    /// warehouse.
    #[test]
    fn a_failed_airhouse_read_is_absent_not_zero() {
        let meters = build_meters(input(None));
        let m = find(&meters, "warehouse");
        assert_eq!(m.quantity, None);
        assert_eq!(m.attribution, Attribution::Unmeasured);
    }

    /// Compute uses the **summed** wall time, not a median times a count.
    /// The two differ by most of the figure on a long-tailed workload, which
    /// is exactly the shape p99-beside-p50 on this surface keeps showing.
    #[test]
    fn compute_uses_summed_wall_time_not_a_quantile_estimate() {
        let t = OrgAppTotals {
            requests: 1_000,
            p50_ms: 1.0,    // a median times the count would give 1,000 ms
            wall_ms: 8_000, // the real total is eight times that
            ..Default::default()
        };
        let meters = build_meters(input(Some(&t)));
        let m = find(&meters, "function_compute");
        // 8,000 ms = 8 s at 128 MiB = 1 GB·s. The p50 estimate would give 0.125.
        assert_eq!(m.quantity, Some(1.0));
    }

    /// **The apportioned class must be reachable.** The first cut always
    /// passed `fleet_wall_ms: None`, so `cost_usd` was `None` on every
    /// deployment — while the note told operators to set
    /// `OXY_FLEET_COMPUTE_USD_MONTH`, which then changed nothing. The env var
    /// was dead config and the page gave an instruction that could not work.
    ///
    /// This pins both halves: with a bill AND a denominator there is a cost,
    /// and the note stops telling anyone to set anything.
    #[test]
    fn with_a_bill_and_a_denominator_compute_is_actually_priced() {
        let t = totals(0, 1_000, 10_000);
        let mut i = input(Some(&t));
        i.fleet_cost = Some(300.0);
        i.fleet_wall_ms = Some(40_000.0);
        let meters = build_meters(i);
        let m = find(&meters, "function_compute");
        assert_eq!(
            m.cost_usd,
            Some(75.0),
            "a quarter of the fleet, a quarter of the bill"
        );
        let note = m.note.as_ref().unwrap();
        assert!(
            !note.contains(rates::FLEET_COST_ENV),
            "nothing left to set: {note}"
        );
        assert!(note.contains("share of the serving fleet"), "{note}");
    }

    /// A denominator without a bill is still no cost — we never invent the
    /// price — and the note names the variable that would supply it.
    #[test]
    fn a_denominator_without_a_bill_still_prices_nothing() {
        let t = totals(0, 1_000, 10_000);
        let mut i = input(Some(&t));
        i.fleet_wall_ms = Some(40_000.0);
        let meters = build_meters(i);
        let m = find(&meters, "function_compute");
        assert_eq!(m.cost_usd, None);
        assert!(m.note.as_ref().unwrap().contains(rates::FLEET_COST_ENV));
    }

    /// An org cannot be charged more than the whole fleet's bill, whatever
    /// the arithmetic does — a share above 1 would mean the denominator was
    /// smaller than the numerator, which is a bug, not a big customer.
    #[test]
    fn an_orgs_share_never_exceeds_the_whole_bill() {
        let t = totals(0, 1, 10_000);
        let mut i = input(Some(&t));
        i.fleet_cost = Some(300.0);
        // The org IS the whole fleet.
        i.fleet_wall_ms = Some(10_000.0);
        let meters = build_meters(i);
        assert_eq!(find(&meters, "function_compute").cost_usd, Some(300.0));
    }

    /// OLTP is named as directly billable but unmeasured — the honest shape.
    /// Leaving it off the page would imply the org does not have one; showing
    /// a guess would be worse.
    #[test]
    fn oltp_is_named_as_billable_but_unmeasured() {
        let meters = build_meters(input(None));
        let m = find(&meters, "oltp");
        assert_eq!(m.attribution, Attribution::Unmeasured);
        assert!(m.note.as_ref().unwrap().contains("Neon"));
        assert!(m.note.as_ref().unwrap().contains("directly billable"));
    }

    /// Every meter that is not `Direct` carries a note. An apportioned or
    /// missing figure without its caveat is the defect this whole model exists
    /// to prevent.
    #[test]
    fn every_non_direct_meter_explains_itself() {
        let t = totals(1, 1, 1);
        for meters in [build_meters(input(None)), build_meters(input(Some(&t)))] {
            for m in &meters {
                if m.attribution != Attribution::Direct {
                    assert!(m.note.is_some(), "{} has no note", m.key);
                }
            }
        }
    }

    /// A disabled heap ceiling leaves compute without a provisioned basis, so
    /// it reports no quantity rather than bare seconds dressed as GB·s.
    #[test]
    fn a_disabled_ceiling_leaves_compute_without_a_quantity() {
        let t = totals(0, 1_000, 8_000);
        let mut i = input(Some(&t));
        i.provisioned_mb = None;
        let meters = build_meters(i);
        let m = find(&meters, "function_compute");
        assert_eq!(m.quantity, None);
        assert!(m.note.as_ref().unwrap().contains("disabled"));
    }
}
