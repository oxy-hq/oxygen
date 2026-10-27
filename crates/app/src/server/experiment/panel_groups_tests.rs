//! Groups of units: membership, the group-summed panel, the additive guard.

use chrono::NaiveDate;
use oxy_airlayer_compat::SemanticLayer;
use oxy_airlayer_compat::engine::UNBOUNDED_QUERY_LIMIT;
use oxy_airlayer_compat::engine::experiment::day_ordinal;
use oxy_airlayer_compat::engine::metric_tree_ops::QueryExecutor;
use oxy_airlayer_compat::engine::query::QueryRequest;
use oxy_experiment::ConfigError;
use oxy_shared::errors::OxyError;
use serde_json::{Map, Value, json};

use super::panel::*;
use super::panel_groups::*;

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
  - name: region
    type: string
    expr: region
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
  - name: revenue
    type: sum
    expr: revenue
  - name: orders
    type: count
  - name: avg_price
    type: average
    expr: price
  - name: revenue_per_order
    type: custom
    expr: "{{store_days.revenue}} / {{store_days.orders}}"
"#;

fn layer() -> SemanticLayer {
    let views = [STORES, STORE_DAYS]
        .iter()
        .map(|y| oxy_airlayer_compat::parse_view_yaml(y).expect("view parses"))
        .collect();
    SemanticLayer::new(views, None)
}

fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

fn membership(groups: &[(&str, &[&str])]) -> GroupMembership {
    GroupMembership(
        groups
            .iter()
            .map(|(g, us)| (g.to_string(), us.iter().map(|u| u.to_string()).collect()))
            .collect(),
    )
}

fn request(measure: &str, m: Option<GroupMembership>) -> PanelRequest {
    PanelRequest {
        entity: "store".to_string(),
        measure: measure.to_string(),
        time_dimension: None,
        grain: oxy_experiment::Grain::Day,
        timezone: None,
        history: HistoryBounds {
            from: None,
            to_exclusive: day(2026, 9, 30),
        },
        membership: m,
        restrict_to: None,
        unit_floor: None,
        filters: vec![],
    }
}

fn group_row(store: Value, region: Value) -> Map<String, Value> {
    json!({ "stores__store_id": store, "stores__region": region })
        .as_object()
        .unwrap()
        .clone()
}

fn panel_row(store: &str, when: &str, revenue: Value) -> Map<String, Value> {
    json!({
        "store_days__store_id": store,
        "store_days__business_date__day": when,
        "store_days__revenue": revenue,
    })
    .as_object()
    .unwrap()
    .clone()
}

fn executor_returning(rows: Vec<Map<String, Value>>) -> Box<QueryExecutor> {
    Box::new(move |_req: &QueryRequest| Ok(rows.clone()))
}

fn spec(assign_by: &str) -> GroupSpec {
    resolve_group_spec(&layer(), "store", assign_by).expect("resolves")
}

#[test]
fn assign_by_resolves_through_the_entity_the_owning_view_or_bare() {
    for written in ["store.region", "stores.region", "region"] {
        let s = spec(written);
        assert_eq!(s.group_dimension, "stores.region", "{written}");
        assert_eq!(s.key_dimension, "stores.store_id", "{written}");
        assert_eq!(s.assign_by, written);
    }
}

#[test]
fn an_assign_by_qualifier_that_is_neither_the_entity_nor_its_view_is_refused() {
    let err = resolve_group_spec(&layer(), "store", "store_days.region").unwrap_err();
    let PanelRefusal::Config(ConfigError::AssignByUnresolved { assign_by, why }) = &err else {
        panic!("{err:?}");
    };
    assert_eq!(assign_by, "store_days.region");
    assert!(
        why.contains("`store_days`") && why.contains("`store`") && why.contains("`stores`"),
        "{why}"
    );
}

#[test]
fn an_assign_by_dimension_the_view_lacks_is_refused_listing_the_dimensions() {
    let err = resolve_group_spec(&layer(), "store", "store.regoin").unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("`regoin`") && message.contains("store_id, region"),
        "{message}"
    );
}

#[test]
fn assigning_by_the_key_itself_is_refused() {
    let err = resolve_group_spec(&layer(), "store", "store.store_id").unwrap_err();
    assert!(
        err.to_string().contains("every unit its own group"),
        "{err}"
    );
}

#[test]
fn the_membership_request_reads_the_owning_view_only_and_unbounded() {
    let q = build_membership_request(&spec("store.region"));
    assert_eq!(
        q.dimensions,
        vec!["stores.store_id".to_string(), "stores.region".to_string()]
    );
    assert!(q.measures.is_empty() && q.time_dimensions.is_empty() && q.filters.is_empty());
    assert_eq!(
        q.limit,
        Some(UNBOUNDED_QUERY_LIMIT),
        "the default 10k cap would drop members"
    );
}

#[test]
fn rows_become_a_membership_with_numeric_keys_rendered_as_strings() {
    let rows = vec![
        group_row(json!(12), json!("east")),
        group_row(json!("bondi"), json!("east")),
        group_row(json!("manly"), json!("west")),
        group_row(json!("manly"), json!("west")), // a repeat of the same pair is not a conflict
    ];
    let m = rows_to_membership(&rows, &spec("store.region")).expect("membership");
    assert_eq!(
        m,
        membership(&[("east", &["12", "bondi"]), ("west", &["manly"])])
    );
}

#[test]
fn a_unit_with_two_group_values_is_refused_naming_it() {
    let rows = vec![
        group_row(json!("bondi"), json!("east")),
        group_row(json!("bondi"), json!("west")),
    ];
    let err = rows_to_membership(&rows, &spec("store.region")).unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("`bondi`") && message.contains("east") && message.contains("west"),
        "{message}"
    );
}

#[test]
fn rows_without_a_key_or_a_group_are_counted_when_none_survive() {
    let rows = vec![
        group_row(Value::Null, json!("east")),
        group_row(json!("bondi"), Value::Null),
    ];
    let err = rows_to_membership(&rows, &spec("store.region")).unwrap_err();
    assert!(
        matches!(
            err,
            PanelRefusal::NoGroups {
                rows: 2,
                skipped: 2,
                ..
            }
        ),
        "{err:?}"
    );
}

#[test]
fn group_sums_add_the_members_per_day_and_drop_non_members() {
    let m = membership(&[("east", &["bondi", "newtown"]), ("west", &["manly"])]);
    let (d1, d2) = (day_ordinal(day(2026, 3, 2)), day_ordinal(day(2026, 3, 3)));
    let triples = vec![
        ("bondi".to_string(), d1, 10.0),
        ("newtown".to_string(), d1, 5.0),
        ("bondi".to_string(), d2, 1.0), // newtown has no row on d2: the sum of the rows that exist
        ("manly".to_string(), d1, 7.0),
        ("cronulla".to_string(), d1, 99.0), // opened after the membership was pinned
    ];
    assert_eq!(
        sum_by_group(triples, &m),
        vec![
            ("east".to_string(), d1, 15.0),
            ("east".to_string(), d2, 1.0),
            ("west".to_string(), d1, 7.0),
        ]
    );
}

#[test]
fn a_sum_and_a_count_target_are_additive() {
    assert!(ensure_additive(&layer(), "store_days.revenue").is_ok());
    assert!(ensure_additive(&layer(), "store_days.orders").is_ok());
}

#[test]
fn a_non_additive_target_under_assign_by_is_refused_naming_the_measure() {
    // Review Focus 6. An average, and a ratio (a `custom`), do not add up.
    for measure in ["store_days.avg_price", "store_days.revenue_per_order"] {
        let err = ensure_additive(&layer(), measure).unwrap_err();
        assert!(
            matches!(&err, PanelRefusal::Config(ConfigError::AssignByNotAdditive { measure: m, .. }) if m == measure),
            "{measure}: {err:?}"
        );
        assert!(err.to_string().contains(&format!("`{measure}`")), "{err}");
    }
}

#[test]
fn the_grouped_panel_has_group_names_as_units() {
    let m = membership(&[("east", &["bondi", "newtown"]), ("west", &["manly"])]);
    let rows = vec![
        panel_row("bondi", "2026-03-02", json!(10.0)),
        panel_row("newtown", "2026-03-02", json!(5.0)),
        panel_row("manly", "2026-03-02", json!(7.0)),
    ];
    let exec = executor_returning(rows);
    let out =
        fetch_panel(&layer(), &*exec, &request("store_days.revenue", Some(m))).expect("outcome");
    let PanelOutcome::Panel(panel) = out else {
        panic!("expected a panel")
    };
    assert_eq!(panel.units(), ["east".to_string(), "west".to_string()]);
}

#[test]
fn a_non_additive_target_comes_back_as_a_refused_value_before_the_warehouse_is_asked() {
    let m = membership(&[("east", &["bondi"])]);
    let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = asked.clone();
    let exec: Box<QueryExecutor> = Box::new(move |_r: &QueryRequest| {
        counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(vec![])
    });
    // A VALUE, not an error: a job that got an `Err` would retry it into the
    // dead-letter queue (index C2a).
    let out = fetch_panel(&layer(), &*exec, &request("store_days.avg_price", Some(m)))
        .expect("a value, not an Err");
    let PanelOutcome::Refused { reason } = out else {
        panic!("expected a refused outcome")
    };
    assert!(
        reason.contains("`store_days.avg_price`") && reason.contains("AVG"),
        "{reason}"
    );
    assert_eq!(
        asked.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the guard must fire before the query"
    );
}

#[test]
fn restrict_to_drops_units_outside_the_list() {
    // A switchback's pinned fleet: a store that opened after the pin is not in it.
    let rows = vec![
        panel_row("bondi", "2026-03-02", json!(10.0)),
        panel_row("manly", "2026-03-02", json!(7.0)),
        panel_row("cronulla", "2026-03-02", json!(99.0)),
    ];
    let exec = executor_returning(rows);
    let mut req = request("store_days.revenue", None);
    req.restrict_to = Some(vec!["bondi".to_string(), "manly".to_string()]);
    let PanelOutcome::Panel(panel) = fetch_panel(&layer(), &*exec, &req).expect("outcome") else {
        panic!("expected a panel");
    };
    assert_eq!(panel.units(), ["bondi".to_string(), "manly".to_string()]);

    // restrict_to never combines with a membership: a refused value, naming both.
    req.membership = Some(membership(&[("east", &["bondi"])]));
    let out = fetch_panel(&layer(), &*exec, &req).expect("a value, not an Err");
    let PanelOutcome::Refused { reason } = out else {
        panic!("expected a refused outcome")
    };
    assert!(
        reason.contains("restrict_to") && reason.contains("membership"),
        "{reason}"
    );
}

#[test]
fn a_panel_sums_by_the_membership_it_is_handed_never_by_the_dimensions_current_value() {
    // Index Review Focus 8, PR 3's half. The warehouse would now say `bondi` is
    // in `east` (its dimension value changed after registration); the pinned
    // membership says `west`. The panel must follow the pinned one, and must
    // not even ask the warehouse which group a unit is in now.
    let pinned = membership(&[("west", &["bondi"])]);
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::<QueryRequest>::new()));
    let log = requests.clone();
    let exec: Box<QueryExecutor> = Box::new(move |r: &QueryRequest| {
        log.lock().unwrap().push(r.clone());
        if r.measures.is_empty() {
            // a membership-style query: it must never be issued
            return Ok(vec![group_row(json!("bondi"), json!("east"))]);
        }
        Ok(vec![panel_row("bondi", "2026-03-02", json!(10.0))])
    });
    let out = fetch_panel(
        &layer(),
        &*exec,
        &request("store_days.revenue", Some(pinned)),
    )
    .expect("outcome");
    let PanelOutcome::Panel(panel) = out else {
        panic!("expected a panel")
    };
    assert_eq!(
        panel.units(),
        ["west".to_string()],
        "the pinned group, not the current one"
    );
    let seen = requests.lock().unwrap();
    assert_eq!(seen.len(), 1, "one panel query and nothing else: {seen:?}");
    assert!(
        seen[0]
            .dimensions
            .iter()
            .all(|d| d == "store_days.store_id"),
        "{:?}",
        seen[0].dimensions
    );
}

#[test]
fn a_unit_reclassified_after_pinning_sums_into_its_pinned_group() {
    // The arithmetic half: the same day's value lands under whichever
    // membership is passed, and a fresh membership would move it.
    let d = day_ordinal(day(2026, 3, 2));
    let triples = vec![
        ("bondi".to_string(), d, 10.0),
        ("manly".to_string(), d, 7.0),
    ];
    let pinned = membership(&[("west", &["bondi"]), ("east", &["manly"])]);
    let fresh = membership(&[("east", &["bondi", "manly"])]);
    assert_eq!(
        sum_by_group(triples.clone(), &pinned),
        vec![("east".to_string(), d, 7.0), ("west".to_string(), d, 10.0)]
    );
    assert_eq!(
        sum_by_group(triples, &fresh),
        vec![("east".to_string(), d, 17.0)]
    );
}

#[test]
fn a_membership_that_matches_no_panel_row_is_refused_with_the_counts() {
    let m = membership(&[("east", &["bondi"])]);
    let exec = executor_returning(vec![panel_row("cronulla", "2026-03-02", json!(3.0))]);
    let err = fetch_panel(&layer(), &*exec, &request("store_days.revenue", Some(m))).unwrap_err();
    let message = err.to_string();
    assert!(message.contains("1 groups"), "{message}");
    assert!(message.contains("store_days.revenue"), "{message}");
}

#[test]
fn zero_rows_are_still_no_history_under_assign_by() {
    let m = membership(&[("east", &["bondi"])]);
    let exec = executor_returning(vec![]);
    let out =
        fetch_panel(&layer(), &*exec, &request("store_days.revenue", Some(m))).expect("outcome");
    assert!(matches!(out, PanelOutcome::NoHistory { .. }));
}

#[test]
fn the_mean_group_size_is_over_the_groups_the_panel_holds() {
    let m = membership(&[
        ("a", &["1", "2", "3", "4"]),
        ("b", &["5", "6"]),
        ("c", &["7"]),
    ]);
    let held = vec!["a".to_string(), "b".to_string()];
    assert_eq!(mean_group_size(&m, &held), Some(3.0));
    assert_eq!(mean_group_size(&m, &["zzz".to_string()]), None);
}

#[test]
fn fetch_groups_runs_the_query_and_maps_it() {
    let exec = executor_returning(vec![group_row(json!("bondi"), json!("east"))]);
    let m = fetch_groups(&layer(), &*exec, "store", "store.region").expect("membership");
    assert_eq!(m, membership(&[("east", &["bondi"])]));
    let refused = fetch_groups(&layer(), &*exec, "store", "store.nope");
    assert!(matches!(refused, Err(OxyError::ValidationError(_))));
}
