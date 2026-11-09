use super::*;
use oxy_simulation::{
    BaselineSpec, CalibrateSpec, DEFAULT_BUDGET_JITTER_SD, EntitiesSpec, LeverSpec, MechanismSpec,
    World,
};

fn spec() -> SimulationSpec {
    SimulationSpec {
        name: "materialize".into(),
        description: None,
        seed: 7,
        replicates: 1,
        periods: 2,
        period_days: 7,
        history_days: 180,
        start_date: chrono::NaiveDate::from_ymd_opt(2025, 1, 6).unwrap(),
        entities: EntitiesSpec {
            count: 24,
            scale_sigma: 0.4,
        },
        baseline: BaselineSpec {
            sales_per_entity_day: 1_500.0,
            margin: 0.36,
            demand_shock_rho: 0.7,
            demand_shock_sd: 0.12,
            weekly_seasonality: 0.15,
            budget_jitter_sd: DEFAULT_BUDGET_JITTER_SD,
        },
        mechanism: MechanismSpec {
            driver: "marketing_spend".into(),
            target: "net_sales".into(),
            lag_days: 7,
            declared_lag_days: None,
            noise_ratio: 0.05,
            calibrate: CalibrateSpec {
                anchor_spend_share: 0.02,
                local_slope_at_anchor: 4.0,
                optimum_at: 3.0,
            },
        },
        lever: LeverSpec::default(),
    }
}

#[test]
fn the_generated_view_parses_as_a_semantic_layer() {
    // The whole reason for materialising real files instead of building a
    // `SemanticLayer` in code is that the *real parser* reads them. If the
    // generated YAML only ever round-tripped through our own writer, that
    // property would be worth nothing.
    let world = WorldDir::create(&spec()).unwrap();
    let layer = oxy_airlayer_compat::load_layer_from_dir(world.root())
        .expect("the generated workspace does not parse");
    let view = layer
        .views
        .iter()
        .find(|v| v.name == TABLE)
        .expect("the generated view is missing from the layer");
    assert_eq!(view.datasource.as_deref(), Some(DATASOURCE));
}

#[test]
fn the_driver_edge_is_left_fittable() {
    // A declared coefficient is never refitted, so an edge that carried one
    // would make the run measure nothing at all — and it would look fine.
    let world = WorldDir::create(&spec()).unwrap();
    let layer = oxy_airlayer_compat::load_layer_from_dir(world.root()).unwrap();
    let tree = oxy_airlayer_compat::engine::metric_tree::MetricTree::build(&layer);
    let roots = vec![format!("{TABLE}.marketing_spend")];
    let fittable = oxy_airlayer_compat::engine::metric_tree_fit::fittable_edges(&tree, &roots);
    assert_eq!(
        fittable.len(),
        1,
        "expected exactly the marketing_spend -> net_sales edge, got {:?}",
        fittable
            .iter()
            .map(|e| (&e.from, &e.to))
            .collect::<Vec<_>>()
    );
}

#[test]
fn appended_rows_land_in_the_csv_with_a_stable_header() {
    let s = spec();
    let world = WorldDir::create(&s).unwrap();
    let mut sink = CsvSink::new(&world);
    let mut engine = World::new(s).unwrap();

    let history = engine.warm_up();
    sink.append(&history).unwrap();
    let after_history = std::fs::read_to_string(world.table_csv()).unwrap();

    let spend = vec![engine.anchor_spend(); engine.entity_count()];
    sink.append(&engine.step(&spend)).unwrap();
    let after_period = std::fs::read_to_string(world.table_csv()).unwrap();

    let header = csv_header(&spec());
    assert!(after_period.starts_with(&header), "header was rewritten");
    assert!(
        after_period.len() > after_history.len(),
        "the period's rows did not append"
    );
    // Header + every row, and nothing blank in between — a stray newline makes
    // DuckDB read a NULL row that the fitter then counts as an observation.
    let lines: Vec<&str> = after_period.lines().collect();
    assert!(lines.iter().all(|l| !l.trim().is_empty()));
    assert_eq!(lines[0], header.trim_end());
}

#[test]
fn the_dataset_dir_is_removed_with_the_run() {
    // A crashed run must leak nothing — that is the property that made a
    // materialised workspace preferable to registering a real database.
    let path = {
        let world = WorldDir::create(&spec()).unwrap();
        let path = world.root().to_path_buf();
        assert!(path.exists());
        path
    };
    assert!(!path.exists(), "the run's workspace outlived it");
}

#[test]
fn the_shipped_fitter_sizes_the_edge_off_generated_rows() {
    // The slice, end to end and with no database: generate a world, append it
    // to the dataset CSV, and ask the *shipped* fitter what it makes of it.
    // Everything below the `probe()` call is production code — the parser, the
    // engine, the SQL, `fit_driver_coefficients`. What the simulation supplied
    // was the rows and the question.
    use crate::simulation::probe::FitProbe;
    use oxy_simulation::SemanticProbe;

    let s = spec();
    let world_dir = WorldDir::create(&s).unwrap();
    let mut sink = CsvSink::new(&world_dir);
    let mut engine = World::new(s.clone()).unwrap();
    sink.append(&engine.warm_up()).unwrap();

    let mut probe = FitProbe::new(&world_dir, &s).unwrap();
    let result = probe.probe().expect("the fit errored");

    let (edge, fit) = result
        .fits
        .first()
        .expect("the fitter returned no row for the declared edge");
    assert_eq!(edge, "store_days.marketing_spend -> store_days.net_sales");

    let beta = fit.coefficient.unwrap_or_else(|| {
        panic!(
            "the fit was refused on {} pairs across {} panels: {:?}",
            fit.n, fit.n_panels, fit.refusal
        )
    });

    // The chain ran: every pair, every panel, a number rather than a refusal.
    assert!(
        beta.is_finite() && beta != 0.0,
        "fitted a degenerate {beta}"
    );
    assert!(fit.n > 4_000, "only {} pairs reached the fit", fit.n);
    assert_eq!(
        fit.n_panels, s.entities.count as usize,
        "the fit did not see every panel — it degraded to a pooled regression"
    );
    assert!(fit.se > 0.0, "a fit with no standard error cannot be gated");

    // And the finding that cost a factor of ~43 to notice. This world declares
    // no `form:`, so the fitter infers one by AIC — and on a saturating world it
    // picks log-log, making the coefficient an *elasticity*. Read as a level
    // slope that is wrong by `target / driver`, which is exactly the gap that
    // first looked like the shipped fitter disagreeing with the scorer.
    assert_eq!(
        fit.form_name, "log-log",
        "the inferred basis changed; the units this test reasons about have moved"
    );
    assert_eq!(fit.form, oxy_simulation::FitForm::NonLinear);
    assert!(
        (beta - 0.09).abs() < 0.02,
        "the raw coefficient {beta} is no longer the elasticity this test pins"
    );

    // The fix: the marginal effect is read off the sampled response, not
    // converted per-form. It must land on the same number the independent
    // within-panel OLS measures — two estimators that were written to be
    // different, agreeing on a world whose answer we chose.
    let marginal = fit
        .level_slope()
        .expect("no marginal effect could be read from the profile");
    let independent = oxy_simulation::check(&s).unwrap();
    let disagreement = (marginal / independent.observational_slope - 1.0).abs();
    // 25%, not 5%. The profile is a *sampled* response over a wide relative
    // range, so a finite difference across two of its samples is a secant, not
    // the tangent at the operating point — on this world that reads 4.62 where
    // the independent tangent estimator reads 3.94. A real approximation with a
    // known sign, not a defect, and far inside the 43x it replaced.
    assert!(
        disagreement < 0.25,
        "the shipped fitter reads {marginal} where the independent estimator gets {} \
         ({:.0}% apart) — they measured different things",
        independent.observational_slope,
        disagreement * 100.0
    );

    // ...and both should sit near the truth, overstated by the confounding the
    // legacy budget rule introduces. The gap is the finding, not a defect, so
    // this is a sanity band rather than a tolerance.
    let bias = marginal / independent.true_local_slope;
    assert!(
        bias > 0.8 && bias < 1.5,
        "marginal {marginal} against a true local slope of {} is not a plausible sizing",
        independent.true_local_slope
    );
}

#[test]
fn a_world_can_name_its_driver_and_target_anything() {
    // `mechanism.driver`/`.target` are not required to be `marketing_spend`/
    // `net_sales` — that pair is just what every shipped `.simulation.yml`
    // happens to call them. The CSV header, the view's measure names, and the
    // fit's roots all derive from whatever a world declares, so a world
    // calling the same mechanism `ad_spend -> signups` must run identically:
    // no crash, no mislabeling, a real fit on a real column.
    let mut s = spec();
    s.mechanism.driver = "ad_spend".into();
    s.mechanism.target = "signups".into();

    let world_dir = WorldDir::create(&s).unwrap();
    let mut sink = CsvSink::new(&world_dir);
    let mut engine = World::new(s.clone()).unwrap();
    sink.append(&engine.warm_up()).unwrap();

    let header = csv_header(&s);
    assert_eq!(
        std::fs::read_to_string(world_dir.table_csv())
            .unwrap()
            .lines()
            .next()
            .unwrap(),
        header.trim_end(),
        "the CSV header did not follow the declared names"
    );

    use crate::simulation::probe::FitProbe;
    use oxy_simulation::SemanticProbe;
    let mut probe = FitProbe::new(&world_dir, &s).unwrap();
    let result = probe.probe().expect("the fit errored");
    let (edge, fit) = result
        .fits
        .first()
        .expect("the fitter returned no row for the declared edge");
    assert_eq!(edge, "store_days.ad_spend -> store_days.signups");
    assert!(
        fit.coefficient.is_some_and(f64::is_finite),
        "renamed driver/target should fit exactly as the default names do: {:?}",
        fit.refusal
    );
}

#[test]
fn the_view_declares_the_guessed_lag_not_the_true_one() {
    // `lag:` is a human claim on real data, and the world it is generated from
    // knows the truth. Writing the true lag here would make the customer right
    // by construction and put the whole lag-error axis out of reach — the fit
    // would pair on exactly the offset the mechanism uses, every time.
    let mut s = spec();
    s.mechanism.declared_lag_days = Some(3);
    let world = WorldDir::create(&s).unwrap();

    let view = std::fs::read_to_string(
        world
            .root()
            .join("semantics/views")
            .join(format!("{TABLE}.view.yml")),
    )
    .expect("read the generated view");
    assert!(
        view.contains("lag: 3"),
        "the view declares the true lag, so the fitter cannot be wrong about it:\n{view}"
    );
    assert!(!view.contains("lag: 7"));
}

#[test]
fn dropping_a_world_dir_releases_its_pooled_dataset_handle() {
    // Runs the production sequence — materialise, check the dataset out of the
    // DuckDB pool, drop — against the real pool, so a release that names the
    // wrong path (or panics) fails here.
    //
    // What it deliberately does NOT assert is the pool's slot count afterwards:
    // that state is `#[cfg(test)]`-private to the `oxy` crate and is not
    // reachable from `oxy-app` without widening `oxy`'s public API for a test.
    // The counting assertions live where the pool does, in
    // `connector::duckdb::tests::per_run_dataset_dirs_do_not_accumulate_pooled_handles`,
    // which releases *before* removing the directory — the ordering this drop
    // depends on, since a path that no longer canonicalizes falls back to its
    // raw form and stops matching the key it was checked out under.
    let world = WorldDir::create(&spec()).unwrap();
    let dataset = world.dataset_dir();
    // The header alone is a legal CSV but gives DuckDB nothing to sniff types
    // from; a run has appended a period by the time it probes.
    let mut sink = CsvSink::new(&world);
    sink.append(&[EntityDay {
        entity_id: 1,
        date: chrono::NaiveDate::from_ymd_opt(2025, 1, 6).unwrap(),
        net_sales: 1_500.0,
        marketing_spend: 30.0,
        prime_cost: 960.0,
    }])
    .unwrap();

    let conn = oxy::connector::checkout_local_connection(dataset.to_str().unwrap())
        .expect("the run's dataset directory must be checkout-able");
    let rows: i64 = conn
        .query_row(&format!("SELECT count(*) FROM {TABLE}"), [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 1);
    drop(conn);

    drop(world);
    assert!(
        !dataset.exists(),
        "the TempDir must still be what cleans the directory up — the Drop impl adds a \
         pool release, it does not take over the deletion"
    );
}
