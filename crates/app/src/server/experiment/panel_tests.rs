//! Unit tests for the panel query. The layer is real (parsed from view YAML);
//! the warehouse is the one external boundary and is a closure.

use chrono::NaiveDate;
use oxy_airlayer_compat::SemanticLayer;
use oxy_airlayer_compat::engine::UNBOUNDED_QUERY_LIMIT;
use oxy_airlayer_compat::engine::experiment::day_ordinal;
use oxy_airlayer_compat::engine::metric_tree_ops::QueryExecutor;
use oxy_airlayer_compat::engine::query::{FilterOperator, QueryRequest};
use oxy_experiment::ConfigError;
use oxy_shared::errors::OxyError;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::panel::*;

const STORES: &str = r#"
name: stores
datasource: warehouse
table: stores
entities:
  - name: store
    type: primary
    key: store_id
dimensions:
  - name: store_id
    type: string
    expr: store_id
measures:
  - name: store_count
    type: count
"#;

const STORE_DAYS: &str = r#"
name: store_days
datasource: warehouse
table: store_days
entities:
  - name: store_day
    type: primary
    key: store_day_id
  - name: store
    type: foreign
    key: store_id
dimensions:
  - name: business_date
    type: date
    expr: business_date
  - name: store_day_id
    type: string
    expr: store_day_id
  - name: store_id
    type: string
    expr: store_id
measures:
  - name: beverages_revenue
    type: sum
    expr: beverages_revenue
"#;

fn layer(views: &[&str]) -> SemanticLayer {
    let views = views
        .iter()
        .map(|y| oxy_airlayer_compat::parse_view_yaml(y).expect("view parses"))
        .collect();
    SemanticLayer::new(views, None)
}

fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

fn request() -> PanelRequest {
    PanelRequest {
        entity: "store".to_string(),
        measure: "store_days.beverages_revenue".to_string(),
        time_dimension: None,
        grain: oxy_experiment::Grain::Day,
        timezone: None,
        history: HistoryBounds {
            from: None,
            to_exclusive: day(2026, 9, 30),
        },
        membership: None,
        restrict_to: None,
        unit_floor: None,
        filters: vec![],
    }
}

fn spec() -> PanelSpec {
    resolve_spec(&layer(&[STORES, STORE_DAYS]), &request()).expect("spec resolves")
}

fn row(store: Value, when: Value, revenue: Value) -> Map<String, Value> {
    json!({
        "store_days__store_id": store,
        "store_days__business_date__day": when,
        "store_days__beverages_revenue": revenue,
    })
    .as_object()
    .unwrap()
    .clone()
}

fn executor_returning(rows: Vec<Map<String, Value>>) -> Box<QueryExecutor> {
    Box::new(move |_req: &QueryRequest| Ok(rows.clone()))
}

#[test]
fn zero_rows_refuse_before_any_triple_is_built() {
    // Review Focus 1, the fast twin. An empty slice never reaches
    // `from_triples`: the outcome is the refusal value.
    let out = rows_to_outcome(&[], &spec()).expect("no refusal error, an outcome");
    assert!(matches!(
        out,
        PanelOutcome::NoHistory { ref measure, ref entity }
            if measure == "store_days.beverages_revenue" && entity == "store"
    ));
}

#[test]
fn an_empty_executor_result_is_no_history_not_a_panel() {
    let exec = executor_returning(vec![]);
    let out = fetch_panel(&layer(&[STORES, STORE_DAYS]), &*exec, &request()).expect("outcome");
    assert!(matches!(out, PanelOutcome::NoHistory { .. }));
}

#[test]
fn rows_with_no_usable_key_day_or_value_are_refused_with_the_counts() {
    let rows = vec![
        row(Value::Null, json!("2026-03-02"), json!(1.0)),
        row(json!("bondi"), json!("not a date"), json!(1.0)),
        row(json!("bondi"), json!("2026-03-02"), Value::Null),
    ];
    let err = rows_to_triples(&rows, &spec()).unwrap_err();
    assert!(
        matches!(
            err,
            PanelRefusal::NoUsableRows {
                rows: 3,
                skipped_key: 1,
                skipped_day: 1,
                skipped_value: 1,
                ..
            }
        ),
        "{err:?}"
    );
}

#[test]
fn numeric_keys_render_as_strings_and_days_as_ordinals() {
    let rows = vec![
        row(json!(12), json!("2026-03-02T00:00:00"), json!(155.5)),
        row(json!("Bondi"), json!("2026-03-02"), json!("20.25")),
    ];
    let triples = rows_to_triples(&rows, &spec()).expect("triples");
    let d = day_ordinal(day(2026, 3, 2));
    // "Bondi" keeps its case: units are matched by exact name.
    assert_eq!(
        triples,
        vec![
            ("12".to_string(), d, 155.5),
            ("Bondi".to_string(), d, 20.25)
        ]
    );
}

#[test]
fn an_integral_key_the_warehouse_typed_as_a_float_renders_without_a_fraction() {
    // DuckDB and the Arrow bridge can hand an integer key over as `12.0`; the
    // unit is still "12", the name lever automations receive in {unit, units}.
    let rows = vec![
        row(json!(12.0), json!("2026-03-02"), json!(1.0)),
        row(json!(-3.0), json!("2026-03-02"), json!(1.0)),
        row(json!(2.5), json!("2026-03-02"), json!(1.0)),
    ];
    let triples = rows_to_triples(&rows, &spec()).expect("triples");
    let units: Vec<&str> = triples.iter().map(|t| t.0.as_str()).collect();
    assert_eq!(units, vec!["12", "-3", "2.5"]);
}

#[test]
fn the_key_dimension_is_the_measure_views_own_when_it_names_the_entity() {
    assert_eq!(spec().key_dimension, "store_days.store_id");
    assert_eq!(spec().time_dimension, "store_days.business_date");
}

#[test]
fn the_key_dimension_falls_back_to_the_owning_view_when_the_measure_view_does_not_name_it() {
    let unlinked = STORE_DAYS.replace(
        "  - name: store\n    type: foreign\n    key: store_id\n",
        "",
    );
    let s = resolve_spec(&layer(&[STORES, &unlinked]), &request()).expect("resolves");
    assert_eq!(s.key_dimension, "stores.store_id");
}

#[test]
fn a_composite_key_entity_is_refused_with_the_config_error() {
    let composite = STORES.replace("    key: store_id", "    keys: [store_id, region]");
    let err = resolve_spec(&layer(&[&composite, STORE_DAYS]), &request()).unwrap_err();
    assert_eq!(
        err,
        PanelRefusal::Config(ConfigError::CompositeKey {
            entity: "store".to_string(),
            keys: vec!["store_id".to_string(), "region".to_string()],
        })
    );
}

#[test]
fn an_entity_no_view_declares_primary_is_refused() {
    let mut req = request();
    req.entity = "warehouse".to_string();
    let err = resolve_spec(&layer(&[STORES, STORE_DAYS]), &req).unwrap_err();
    assert_eq!(err, PanelRefusal::UnknownEntity("warehouse".to_string()));
}

#[test]
fn an_entity_declared_primary_in_two_views_is_refused_naming_both() {
    let twin = STORES.replace("name: stores", "name: stores_copy");
    let err = resolve_spec(&layer(&[STORES, &twin, STORE_DAYS]), &request()).unwrap_err();
    assert!(
        matches!(err, PanelRefusal::AmbiguousEntity { ref views, .. } if views.len() == 2),
        "{err:?}"
    );
}

#[test]
fn a_measure_the_model_does_not_declare_is_refused() {
    let mut req = request();
    req.measure = "store_days.no_such_measure".to_string();
    let err = resolve_spec(&layer(&[STORES, STORE_DAYS]), &req).unwrap_err();
    assert_eq!(
        err,
        PanelRefusal::UnknownMeasure("store_days.no_such_measure".to_string())
    );
}

fn two_dates() -> String {
    STORE_DAYS.replace(
        "measures:",
        "  - name: created_at\n    type: datetime\n    expr: created_at\nmeasures:",
    )
}

#[test]
fn an_absent_time_dimension_uses_the_views_only_date_dimension() {
    assert_eq!(spec().time_dimension, "store_days.business_date");
}

#[test]
fn an_explicit_time_dimension_is_used_even_when_the_view_has_several() {
    let mut req = request();
    req.time_dimension = Some("store_days.created_at".to_string());
    let s = resolve_spec(&layer(&[STORES, &two_dates()]), &req).expect("resolves");
    assert_eq!(s.time_dimension, "store_days.created_at");
}

#[test]
fn an_explicit_time_dimension_that_is_not_a_date_dimension_of_the_measure_view_is_refused() {
    for given in [
        "store_days.store_id",
        "stores.store_id",
        "store_days.no_such",
    ] {
        let mut req = request();
        req.time_dimension = Some(given.to_string());
        let err = resolve_spec(&layer(&[STORES, STORE_DAYS]), &req).unwrap_err();
        assert!(
            matches!(
                err,
                PanelRefusal::TimeDimensionNotADate { given: ref g, ref found, .. }
                    if g == given && found == &vec!["store_days.business_date".to_string()]
            ),
            "{given}: {err:?}"
        );
        let message = err.to_string();
        assert!(
            message.contains(given) && message.contains("store_days.business_date"),
            "{message}"
        );
    }
}

#[test]
fn an_absent_time_dimension_with_several_date_dimensions_is_refused_naming_the_key_and_both() {
    let err = resolve_spec(&layer(&[STORES, &two_dates()]), &request()).unwrap_err();
    assert!(
        matches!(
            err,
            PanelRefusal::TimeDimensionAmbiguous { ref found, .. }
                if found == &vec!["store_days.business_date".to_string(), "store_days.created_at".to_string()]
        ),
        "{err:?}"
    );
    let message = err.to_string();
    assert!(
        message.contains("`design.time_dimension`") && message.contains("store_days.created_at"),
        "{message}"
    );
}

#[test]
fn an_absent_time_dimension_with_no_date_dimension_is_refused() {
    let none = STORE_DAYS.replace("type: date\n", "type: string\n");
    let err = resolve_spec(&layer(&[STORES, &none]), &request()).unwrap_err();
    assert!(
        matches!(err, PanelRefusal::NoTimeDimension { .. }),
        "{err:?}"
    );
}

#[test]
fn refusals_name_the_measure_being_read_never_outcome_target() {
    // PR 4 reuses `resolve_spec` for read-back and guardrail measures, so a
    // message that said `outcome.target` would be wrong there.
    let mut unknown = request();
    unknown.measure = "store_days.refund_rate".to_string();
    let mut given = request();
    given.time_dimension = Some("store_days.store_id".to_string());
    let refusals = [
        resolve_spec(&layer(&[STORES, STORE_DAYS]), &unknown).unwrap_err(),
        resolve_spec(&layer(&[STORES, STORE_DAYS]), &given).unwrap_err(),
        resolve_spec(&layer(&[STORES, &two_dates()]), &request()).unwrap_err(),
        resolve_spec(
            &layer(&[
                STORES,
                &STORE_DAYS.replace("type: date\n", "type: string\n"),
            ]),
            &request(),
        )
        .unwrap_err(),
    ];
    for err in refusals {
        let message = err.to_string();
        assert!(!message.contains("outcome.target"), "{message}");
        assert!(message.contains("the measure `store_days."), "{message}");
    }
}

#[test]
fn the_request_is_unbounded_bounded_by_history_and_grouped_by_day() {
    let bounds = HistoryBounds {
        from: Some(day(2025, 1, 1)),
        to_exclusive: day(2026, 9, 30),
    };
    let q = build_request(&spec(), &bounds);
    assert_eq!(
        q.limit,
        Some(UNBOUNDED_QUERY_LIMIT),
        "the default 10k cap would truncate the panel"
    );
    assert_eq!(q.measures, vec!["store_days.beverages_revenue".to_string()]);
    assert_eq!(q.dimensions, vec!["store_days.store_id".to_string()]);
    assert_eq!(q.time_dimensions.len(), 1);
    assert_eq!(q.time_dimensions[0].granularity.as_deref(), Some("day"));
    let ops: Vec<_> = q
        .filters
        .iter()
        .map(|f| (f.operator.clone(), f.values.clone()))
        .collect();
    assert!(
        ops.iter()
            .any(|(op, v)| matches!(op, Some(FilterOperator::BeforeDate))
                && v == &vec!["2026-09-30".to_string()])
    );
    assert!(
        ops.iter()
            .any(|(op, v)| matches!(op, Some(FilterOperator::AfterOrOnDate))
                && v == &vec!["2025-01-01".to_string()])
    );
}

#[test]
fn a_lower_bound_is_optional() {
    let q = build_request(&spec(), &request().history);
    assert_eq!(
        q.filters.len(),
        1,
        "only the upper bound: {:?}",
        q.filters.len()
    );
}

#[test]
fn model_refusals_are_validation_errors_and_a_failed_query_is_not() {
    let mut req = request();
    req.entity = "warehouse".to_string();
    let refused = fetch_panel(
        &layer(&[STORES, STORE_DAYS]),
        &*executor_returning(vec![]),
        &req,
    );
    assert!(matches!(refused, Err(OxyError::ValidationError(_))));

    let failing: Box<QueryExecutor> = Box::new(|_r: &QueryRequest| {
        Err(oxy_airlayer_compat::engine::EngineError::QueryError(
            "connection reset".to_string(),
        ))
    });
    let broken = fetch_panel(&layer(&[STORES, STORE_DAYS]), &*failing, &request());
    assert!(matches!(broken, Err(OxyError::RuntimeError(ref m)) if m.contains("connection reset")));
}

#[test]
fn a_workspace_that_moved_to_another_revision_is_refused() {
    let asked = Uuid::from_u128(1);
    let now_promoted = Uuid::from_u128(2);
    assert!(check_revision(Some(asked), asked).is_ok());
    let err = check_revision(Some(now_promoted), asked).unwrap_err();
    assert!(matches!(err, PanelRefusal::Revision { .. }), "{err:?}");
    assert!(
        err.to_string().contains(&asked.to_string())
            && err.to_string().contains(&now_promoted.to_string())
    );
    assert!(matches!(
        check_revision(None, asked),
        Err(PanelRefusal::Revision { .. })
    ));
}

#[test]
fn a_measure_filter_narrows_the_panel_query_on_its_dimension() {
    use oxy_experiment::{FilterOp, MeasureFilter};
    let mut req = request();
    req.filters = vec![MeasureFilter {
        dimension: "store_days.store_id".into(),
        op: FilterOp::NotEquals,
        values: vec!["bondi".into(), "manly".into()],
    }];
    let spec = resolve_spec(&layer(&[STORES, STORE_DAYS]), &req).expect("spec resolves");
    let q = build_request(&spec, &req.history);
    let f = q
        .filters
        .iter()
        .find(|f| f.member.as_deref() == Some("store_days.store_id"))
        .expect("the filter reaches the query");
    assert_eq!(f.operator, Some(FilterOperator::NotEquals));
    assert_eq!(f.values, vec!["bondi".to_string(), "manly".to_string()]);
}

#[test]
fn a_filter_on_a_dimension_the_view_lacks_is_refused_by_name() {
    use oxy_experiment::{FilterOp, MeasureFilter};
    let mut req = request();
    req.filters = vec![MeasureFilter {
        dimension: "store_days.sku".into(),
        op: FilterOp::Equals,
        values: vec!["TB-40".into()],
    }];
    let err = resolve_spec(&layer(&[STORES, STORE_DAYS]), &req).unwrap_err();
    assert!(
        matches!(&err, PanelRefusal::UnknownFilterDimension { dimension, view }
            if dimension == "store_days.sku" && view == "store_days"),
        "{err:?}"
    );
}

// ── grain ───────────────────────────────────────────────────────────────────

fn week_spec() -> PanelSpec {
    let mut req = request();
    req.grain = oxy_experiment::Grain::Week;
    resolve_spec(&layer(&[STORES, STORE_DAYS]), &req).expect("spec resolves")
}

fn week_row(store: &str, when: &str, revenue: f64) -> Map<String, Value> {
    json!({
        "store_days__store_id": store,
        "store_days__business_date__week": when,
        "store_days__beverages_revenue": revenue,
    })
    .as_object()
    .unwrap()
    .clone()
}

#[test]
fn a_week_grain_request_groups_by_week_and_bounds_on_whole_weeks() {
    let bounds = HistoryBounds {
        // A Thursday and a Wednesday: the weeks they fall in start 2025-12-29 and 2026-09-28.
        from: Some(day(2026, 1, 1)),
        to_exclusive: day(2026, 9, 30),
    };
    let q = build_request(&week_spec(), &bounds);
    assert_eq!(q.time_dimensions[0].granularity.as_deref(), Some("week"));
    let values: Vec<_> = q.filters.iter().map(|f| f.values.clone()).collect();
    assert!(
        values.contains(&vec!["2026-09-28".to_string()]),
        "{values:?}"
    );
    assert!(
        values.contains(&vec!["2025-12-29".to_string()]),
        "{values:?}"
    );
}

#[test]
fn week_rows_become_consecutive_week_ordinals() {
    let rows = vec![
        week_row("a", "2026-09-07T00:00:00", 1.0),
        week_row("a", "2026-09-14T00:00:00", 2.0),
    ];
    let t = rows_to_triples(&rows, &week_spec()).unwrap();
    assert_eq!(t[1].1 - t[0].1, 1);
    assert_eq!(t[0].1, oxy_experiment::Grain::Week.ordinal(day(2026, 9, 7)));
}

#[test]
fn a_week_bucket_that_does_not_start_on_monday_is_refused_naming_its_day() {
    // BigQuery and MySQL start weeks on Sunday.
    let rows = vec![week_row("a", "2026-09-06", 1.0)];
    let err = rows_to_triples(&rows, &week_spec()).unwrap_err();
    assert!(
        matches!(err, PanelRefusal::PeriodNotAligned { .. }),
        "{err:?}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("2026-09-06") && msg.contains("Sunday"),
        "{msg}"
    );
}

#[test]
fn hour_rows_become_consecutive_hour_ordinals_across_midnight() {
    // Built directly: resolving refuses hour grain on this view's date dimension.
    let mut spec = spec();
    spec.grain = oxy_experiment::Grain::Hour;
    let hour_row = |when: &str| {
        json!({
            "store_days__store_id": "a",
            "store_days__business_date__hour": when,
            "store_days__beverages_revenue": 1.0,
        })
        .as_object()
        .unwrap()
        .clone()
    };
    let rows = vec![
        hour_row("2026-09-07T23:00:00"),
        hour_row("2026-09-08 00:00:00"),
    ];
    let t = rows_to_triples(&rows, &spec).unwrap();
    assert_eq!(t[1].1 - t[0].1, 1);
}

#[test]
fn hour_grain_on_a_date_dimension_is_refused_naming_it() {
    let mut req = request();
    req.grain = oxy_experiment::Grain::Hour;
    let err = resolve_spec(&layer(&[STORES, STORE_DAYS]), &req).unwrap_err();
    assert!(
        err.to_string().contains("store_days.business_date")
            && err.to_string().contains("datetime"),
        "{err}"
    );
}

#[test]
fn an_hour_grain_request_buckets_hours_in_the_design_time_zone() {
    let mut req = request();
    req.grain = oxy_experiment::Grain::Hour;
    req.timezone = Some("Australia/Sydney".to_string());
    let mut spec = spec();
    spec.grain = req.grain;
    spec.timezone = req.timezone.clone();
    let q = build_request(&spec, &req.history);
    assert_eq!(q.time_dimensions[0].granularity.as_deref(), Some("hour"));
    assert_eq!(q.timezone.as_deref(), Some("Australia/Sydney"));
    // Coarser grains keep the warehouse's own bucketing, as before.
    assert_eq!(
        build_request(&super::panel_tests::spec(), &req.history).timezone,
        None
    );
}

#[test]
fn with_a_unit_floor_a_sparse_unit_is_dropped_and_listed_instead_of_everyones_days() {
    // bondi has all three days; newtown only the first: without a floor day 2
    // and day 3 would vanish for bondi too.
    let rows = vec![
        row(json!("bondi"), json!("2026-03-02"), json!(1.0)),
        row(json!("bondi"), json!("2026-03-03"), json!(2.0)),
        row(json!("bondi"), json!("2026-03-04"), json!(3.0)),
        row(json!("manly"), json!("2026-03-02"), json!(1.0)),
        row(json!("manly"), json!("2026-03-03"), json!(2.0)),
        row(json!("manly"), json!("2026-03-04"), json!(3.0)),
        row(json!("newtown"), json!("2026-03-02"), json!(1.0)),
    ];
    let exec = executor_returning(rows);
    let mut req = request();
    req.unit_floor = Some(0.9);
    let out = fetch_panel(&layer(&[STORES, STORE_DAYS]), &*exec, &req).expect("outcome");
    let PanelOutcome::Thinned { panel, dropped } = out else {
        panic!("a floor thins the panel, got {out:?}");
    };
    assert_eq!(panel.units().to_vec(), vec!["bondi", "manly"]);
    assert_eq!(panel.n_days(), 3);
    assert_eq!(dropped.len(), 1);
    assert_eq!(dropped[0].unit, "newtown");
}

#[test]
fn without_a_unit_floor_the_panel_is_as_before() {
    let rows = vec![
        row(json!("bondi"), json!("2026-03-02"), json!(1.0)),
        row(json!("bondi"), json!("2026-03-03"), json!(2.0)),
        row(json!("newtown"), json!("2026-03-02"), json!(1.0)),
    ];
    let exec = executor_returning(rows);
    let out = fetch_panel(&layer(&[STORES, STORE_DAYS]), &*exec, &request()).expect("outcome");
    let PanelOutcome::Panel(m) = out else {
        panic!("no floor, a plain panel");
    };
    assert_eq!(m.n_days(), 1, "the incomplete day goes for every unit");
}
