use oxy_airlayer_compat::engine::experiment::{PRE_FIT_CEILING, PanelMatrix, SyntheticSpec};

use super::synthetic_estimate::estimate_synthetic_target;
use super::target_estimate::Read;

/// Twelve donors on two shared factors, and `t0` a mix of the first three at a
/// level far above them, gaining 25 from day 90.
fn panel() -> PanelMatrix {
    let f1 = |t: i64| (t as f64 * 0.21).sin() * 30.0;
    let f2 = |t: i64| [0.0, 4.0, 9.0, 7.0, 3.0, -8.0, -15.0][(t % 7) as usize] * 3.0;
    let noise = |u: i64, t: i64| ((u * 31 + t * 17) as f64 * 0.7).sin() * 2.0;
    let load = |u: i64| (0.5 + (u % 4) as f64 * 0.3, 1.4 - (u % 3) as f64 * 0.3);
    let mut rows = Vec::new();
    for u in 0..12 {
        let (a, b) = load(u);
        for t in 1..=120 {
            let v = 100.0 + 40.0 * u as f64 + a * f1(t) + b * f2(t) + noise(u, t);
            rows.push((format!("d{u:02}"), t, v));
        }
    }
    let (a, b) = (0..3).fold((0.0, 0.0), |(x, y), u| {
        (x + load(u).0 / 3.0, y + load(u).1 / 3.0)
    });
    for t in 1..=120 {
        let lift = if t >= 90 { 25.0 } else { 0.0 };
        rows.push((
            "t0".into(),
            t,
            5000.0 + a * f1(t) + b * f2(t) + noise(99, t) + lift,
        ));
    }
    PanelMatrix::from_triples(rows)
}

fn spec(treated: &[&str], donors: &[&str]) -> SyntheticSpec {
    let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    SyntheticSpec {
        treated: names(treated),
        start_on: 90,
        donors: Some(names(donors)),
        exclude: names(&["gone"]),
        pre_days: 56,
        post_days: 21,
        anticipation_days: 0,
        washout_days: 0,
        coverage_floor: 0.9,
        alpha: 0.1,
        family: 1,
        max_pre_fit: PRE_FIT_CEILING,
    }
}

const DONORS: [&str; 12] = [
    "d00", "d01", "d02", "d03", "d04", "d05", "d06", "d07", "d08", "d09", "d10", "d11",
];

#[test]
fn the_effect_and_each_fit_read_back_and_the_raw_json_keeps_the_fits() {
    let r = estimate_synthetic_target(&panel(), &spec(&["t0"], &DONORS), 7, Read::STRICT);
    let e = &r.effect.view;
    assert!(e.refusal.is_none(), "{:?}", e.refusal);
    assert!((e.estimate.unwrap() - 25.0).abs() < 6.0, "{:?}", e.estimate);
    let fits = r.fits.expect("fits on an estimate that did not refuse");
    assert_eq!(fits.view.fits[0].unit, "t0");
    assert_eq!(fits.view.donors.len(), 12);
    assert_eq!(fits.view.fits[0].series.len(), 56 + 21);
    assert!(
        r.effect.raw["synthetic"]["fits"].is_array(),
        "kept for audit"
    );
}

#[test]
fn a_reading_of_past_data_lists_a_treated_unit_and_a_donor_the_panel_lacks() {
    let mut donors = DONORS.to_vec();
    donors.push("d99");
    let lenient = Read {
        lenient: true,
        ..Read::STRICT
    };
    let r = estimate_synthetic_target(&panel(), &spec(&["t0", "t9"], &donors), 7, lenient);
    let e = &r.effect.view;
    assert!(e.refusal.is_none(), "{:?}", e.refusal);
    let dropped: Vec<&str> = r.effect.raw["dropped_units"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["unit"].as_str().unwrap())
        .collect();
    assert_eq!(dropped, ["t9", "d99"]);
}

#[test]
fn the_strict_path_lets_the_engine_refuse_a_missing_treated_unit() {
    let r = estimate_synthetic_target(&panel(), &spec(&["t0", "t9"], &DONORS), 7, Read::STRICT);
    assert!(r.effect.view.refusal.is_some());
    assert!(r.fits.is_none(), "a refused estimate has no fits to draw");
}
