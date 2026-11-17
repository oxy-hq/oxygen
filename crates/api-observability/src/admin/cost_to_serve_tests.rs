use super::*;

fn basis() -> Basis {
    Basis {
        days: 30,
        apps_measured: true,
        include_storage: true,
        fleet_wall_ms: None,
        fleet_bill: None,
    }
}

fn priced(wall_ms: f64) -> Basis {
    Basis {
        fleet_wall_ms: Some(wall_ms),
        fleet_bill: Some(1_000.0),
        ..basis()
    }
}

fn storage(gib: f64, apps_with_samples: usize) -> storage_rollup::OrgStorage {
    storage_rollup::OrgStorage {
        average_bytes: gib * 1_073_741_824.0,
        gib,
        apps_with_samples,
    }
}

fn traffic(bytes: u64, wall_ms: u64) -> OrgAppTotals {
    OrgAppTotals {
        bytes,
        wall_ms,
        ..Default::default()
    }
}

fn org(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

/// Org 1 served 100 ms of the prior fleet's wall time and stored 100 GiB.
fn prior_reads(fleet_wall_ms: Option<f64>) -> PriorReads {
    PriorReads {
        traffic: [(org(1).to_string(), traffic(0, 100))]
            .into_iter()
            .collect(),
        storage: [(org(1), storage(100.0, 1))].into_iter().collect(),
        fleet_wall_ms,
        ..Default::default()
    }
}

// --- cost to serve ---------------------------------------------------------

/// The fleet list prices storage and egress exactly as the Resources panel
/// does, through the same `rates` functions, so an org's row and its console
/// cannot disagree about one window.
#[test]
fn cost_to_serve_prices_each_component_at_its_rate() {
    let c = cost_to_serve(
        10.0,
        Some(&storage(100.0, 2)),
        Some(&traffic(50_000_000_000, 0)),
        &basis(),
    );
    assert_eq!(c.llm_usd, 10.0);
    assert_eq!(c.storage_usd, Some(rates::storage_usd(100.0)));
    assert_eq!(c.egress_usd, Some(rates::egress_usd(50.0)));
    assert_eq!(c.compute_usd, None, "no bill, no compute price");
    assert!((c.total() - (10.0 + 2.3 + 4.5)).abs() < 1e-9);
}

/// Unmeasured is absent, not zero: storage with no readings and egress on a
/// deployment with no app store add nothing *and* say so.
#[test]
fn unmeasured_components_are_absent_not_zero() {
    let unmeasured = Basis {
        apps_measured: false,
        ..basis()
    };
    let c = cost_to_serve(3.0, Some(&storage(100.0, 0)), None, &unmeasured);
    assert_eq!(c.storage_usd, None, "rows with no readings are not a zero");
    assert_eq!(c.egress_usd, None);
    assert_eq!(c.total(), 3.0);

    // Measured and quiet: the org served nothing, which is a zero.
    let c = cost_to_serve(3.0, None, None, &basis());
    assert_eq!(c.egress_usd, Some(0.0));
}

/// A failed storage read leaves storage out of the total for every org,
/// rather than pricing whoever happened to have rows.
#[test]
fn a_failed_storage_read_prices_no_storage() {
    let b = Basis {
        include_storage: false,
        ..basis()
    };
    let c = cost_to_serve(0.0, Some(&storage(100.0, 3)), None, &b);
    assert_eq!(c.storage_usd, None);
}

/// Compute is priced only against an operator-supplied bill, as this org's
/// share of the fleet's wall time.
#[test]
fn compute_is_a_share_of_the_supplied_bill() {
    let c = cost_to_serve(0.0, None, Some(&traffic(0, 100)), &priced(400.0));
    assert_eq!(c.compute_usd, Some(250.0));
}

/// A bill over a fleet that ran nothing prices no compute: the column would
/// otherwise sit empty under a key that claims it is priced.
#[test]
fn a_bill_over_an_idle_fleet_is_not_priced_compute() {
    assert!(priced(400.0).compute_priced());
    assert!(!priced(0.0).compute_priced());
    assert!(!basis().compute_priced(), "no bill");
}

// --- retention -------------------------------------------------------------

/// At the console's 90-day window the window before reaches 180 days back,
/// past the default 90-day run retention and observability TTL. Comparing
/// against it would show the whole fleet as new.
#[test]
fn the_90_day_window_has_no_retained_prior() {
    let h = horizon(Some(90), Some(90));
    assert!(prior_retained(7, h));
    assert!(prior_retained(30, h));
    assert!(prior_retained(45, h), "90 days back is still kept");
    assert!(!prior_retained(90, h));
}

/// The shorter retention bounds it; run retention switched off and the
/// store's "no TTL" both keep everything.
#[test]
fn the_horizon_is_the_shorter_retention() {
    assert_eq!(horizon(Some(365), Some(90)), Some(90));
    assert_eq!(horizon(Some(30), Some(90)), Some(30));
    assert_eq!(horizon(None, None), None);
    assert_eq!(horizon(None, Some(0)), None, "a zero TTL keeps everything");
    assert!(prior_retained(90, horizon(None, None)));
    assert!(!prior_retained(30, horizon(Some(30), None)));
}

// --- the prior window ------------------------------------------------------

/// One failed read of three drops the comparison: two components of three
/// would compare different sums.
#[test]
fn any_failed_prior_read_drops_the_comparison() {
    let ok_cost: Result<Vec<cost_rollup::FleetOrgRow>, String> = Ok(Vec::new());
    let ok_storage: Result<HashMap<Uuid, storage_rollup::OrgStorage>, String> = Ok(HashMap::new());
    let ok_traffic: Result<TrafficRead, String> = Ok(None);

    assert!(gather_prior(ok_cost, ok_storage, ok_traffic).is_some());
    assert!(
        gather_prior(
            Err::<Vec<_>, _>("db"),
            Ok::<_, String>(HashMap::new()),
            Ok::<TrafficRead, String>(None)
        )
        .is_none()
    );
    assert!(
        gather_prior(
            Ok::<Vec<_>, String>(Vec::new()),
            Err::<HashMap<_, _>, _>("db"),
            Ok::<TrafficRead, String>(None)
        )
        .is_none()
    );
    assert!(
        gather_prior(
            Ok::<Vec<_>, String>(Vec::new()),
            Ok::<_, String>(HashMap::new()),
            Err::<TrafficRead, _>("clickhouse")
        )
        .is_none()
    );
}

/// A deployment with no observability store reads no prior traffic: that is
/// a read, not a failure, and the comparison stands.
#[test]
fn an_unmeasured_prior_window_is_still_a_read() {
    let p = gather_prior(
        Ok::<Vec<_>, String>(Vec::new()),
        Ok::<_, String>(HashMap::new()),
        Ok::<TrafficRead, String>(None),
    )
    .expect("read");
    assert!(p.traffic.is_empty());
    assert_eq!(p.fleet_wall_ms, None);
}

/// Compute priced now and priced then: both totals carry it.
#[test]
fn compute_priced_in_both_windows_is_in_both_totals() {
    let totals =
        prior_totals(&[org(1)], &priced(400.0), &prior_reads(Some(200.0))).expect("comparable");
    let storage = rates::storage_usd(100.0);
    // 100 of 200 ms of a $1,000/month bill over 30 days.
    assert!(
        (totals[&org(1)] - (storage + 500.0)).abs() < 1e-6,
        "{totals:?}"
    );
}

/// Priced now, not then — the prior denominator failed (`None`) or the fleet
/// ran nothing then (`Some(0)`): compute would appear from nowhere, so there
/// is no comparison.
#[test]
fn compute_priced_now_and_not_then_is_no_comparison() {
    assert!(prior_totals(&[org(1)], &priced(400.0), &prior_reads(None)).is_none());
    assert!(prior_totals(&[org(1)], &priced(400.0), &prior_reads(Some(0.0))).is_none());
}

/// Not priced now — no bill, or a fleet that ran nothing this window — while
/// it would have been then: compute is left out of the prior total too.
#[test]
fn compute_unpriced_now_is_left_out_of_the_prior_total() {
    let storage = rates::storage_usd(100.0);
    for now in [basis(), priced(0.0)] {
        let totals = prior_totals(&[org(1)], &now, &prior_reads(Some(200.0))).expect("comparable");
        assert!((totals[&org(1)] - storage).abs() < 1e-9, "{totals:?}");
    }
}

/// Storage that could not be read now is left out of the prior total too, or
/// the change would be a change in what is counted.
#[test]
fn storage_failed_now_is_left_out_of_the_prior_total() {
    let now = Basis {
        include_storage: false,
        ..basis()
    };
    let totals = prior_totals(&[org(1), org(2)], &now, &prior_reads(None)).expect("comparable");
    assert_eq!(totals[&org(1)], 0.0);
    assert_eq!(
        totals[&org(2)],
        0.0,
        "an org absent then spent nothing then"
    );
}
