use super::*;
use oxy_observability::types::OrgAppTotals;

/// A measured-but-quiet org reports zero; an unmeasured deployment reports
/// absent. These are different answers and the column header says which.
#[test]
fn measured_and_quiet_is_zero_while_unmeasured_is_absent() {
    assert_eq!(zero_if(true, 0u64), Some(0));
    assert_eq!(zero_if(false, 0u64), None);
}

/// A bounded grant binds each org it may see. The org list is the page's
/// spine — every row comes from it — so a fence that leaked here would
/// list orgs the cost rollup then refuses to populate, and the page would
/// read "these orgs spent nothing" about tenants the caller may not see.
#[test]
fn the_org_list_binds_each_in_scope_org() {
    let mut values: Vec<sea_orm::Value> = Vec::new();
    let sql = visible_orgs_sql(Some(&[Uuid::nil(), Uuid::from_u128(3)]), &mut values);
    assert!(
        sql.contains("WHERE id IN ($1,$2)"),
        "the list takes no window param, so orgs start at $1: {sql}"
    );
    assert_eq!(values.len(), 2, "one bound value per org");
}

/// An unbounded reader gets no `WHERE` at all. `IN ()` would list no orgs,
/// so the whole surface would render as an empty platform.
#[test]
fn an_unbounded_reader_lists_every_org() {
    let sql = visible_orgs_sql(None, &mut Vec::new());
    assert!(!sql.contains("WHERE"), "no fence for an owner: {sql}");
    assert!(sql.contains("ORDER BY name"), "{sql}");
}

/// The list is ordered by name, not by cost. The page ranks by cost to
/// serve — that is the frontend's job and it is interactive — but the
/// API's order must not depend on data, or the same request returns rows
/// in a different order as spend moves, which breaks pagination the day
/// it is added.
#[test]
fn the_api_orders_by_name_not_by_spend() {
    let sql = visible_orgs_sql(None, &mut Vec::new());
    assert!(sql.ends_with("ORDER BY name"), "{sql}");
}

fn basis() -> Basis {
    Basis {
        days: 30,
        apps_measured: true,
        include_storage: true,
        fleet_wall_ms: None,
        fleet_bill: None,
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

fn org(id: u128, name: &str) -> OrgRow {
    OrgRow {
        id: Uuid::from_u128(id),
        name: name.into(),
        slug: name.into(),
    }
}

/// With a prior window read, an org absent from it spent nothing then —
/// a zero, which is what makes it new. Without one, there is no prior at
/// all, and the page draws no comparison.
#[test]
fn a_read_prior_window_zeroes_absent_orgs_and_a_failed_one_is_absent() {
    let traffic = HashMap::new();
    let storage = HashMap::new();
    let current = Window {
        cost_rows: &[],
        traffic: &traffic,
        storage: &storage,
        basis: basis(),
    };
    let prior: HashMap<Uuid, f64> = [(Uuid::from_u128(1), 7.5)].into_iter().collect();

    let rows = fold_fleet(FleetFold {
        orgs: vec![org(1, "a"), org(2, "b")],
        current: &current,
        prior: Some(&prior),
        provisioned_mb: None,
        days: 30,
    });
    assert_eq!(rows[0].prior_cost_to_serve_usd, Some(7.5));
    assert_eq!(rows[1].prior_cost_to_serve_usd, Some(0.0));

    let rows = fold_fleet(FleetFold {
        orgs: vec![org(1, "a")],
        current: &current,
        prior: None,
        provisioned_mb: None,
        days: 30,
    });
    assert_eq!(rows[0].prior_cost_to_serve_usd, None);
}

/// The row's total is the sum of its parts, the parts the page draws.
#[test]
fn the_rows_total_is_its_components() {
    let traffic: HashMap<String, OrgAppTotals> =
        [(Uuid::from_u128(1).to_string(), traffic(2_000_000_000, 0))]
            .into_iter()
            .collect();
    let storage: HashMap<Uuid, storage_rollup::OrgStorage> =
        [(Uuid::from_u128(1), storage(10.0, 1))]
            .into_iter()
            .collect();
    let current = Window {
        cost_rows: &[],
        traffic: &traffic,
        storage: &storage,
        basis: basis(),
    };
    let rows = fold_fleet(FleetFold {
        orgs: vec![org(1, "a")],
        current: &current,
        prior: None,
        provisioned_mb: None,
        days: 30,
    });
    let r = &rows[0];
    let parts = r.cost_usd
        + r.storage_usd.unwrap_or(0.0)
        + r.egress_usd.unwrap_or(0.0)
        + r.compute_usd.unwrap_or(0.0);
    assert!((r.cost_to_serve_usd - parts).abs() < 1e-9);
    assert_eq!(r.storage_gib_month, Some(10.0));
}
