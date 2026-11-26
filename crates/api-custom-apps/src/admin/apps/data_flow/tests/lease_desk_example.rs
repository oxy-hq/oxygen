//! `customer-apps/examples/lease-desk` on the map: its own manifest, as a
//! promote records it, draws what the example's README says lands where.

use super::*;

/// `customer-apps/examples/lease-desk/oxy-app.json`, the example the relay's
/// end-to-end tests publish.
const LEASE_DESK_EXAMPLE: &str =
    include_str!("../../../../../../../customer-apps/examples/lease-desk/oxy-app.json");

/// The example as a promote records it: its manifest, and one function row
/// per `functions` entry.
fn example_inputs(level: Level) -> Inputs {
    let manifest: Value = serde_json::from_str(LEASE_DESK_EXAMPLE).expect("the example parses");
    let functions = manifest["functions"]
        .as_object()
        .expect("the example declares functions")
        .iter()
        .map(|(name, spec)| function(name, spec.clone()))
        .collect();
    let app = AppIn {
        id: APP,
        slug: manifest["slug"].as_str().expect("a slug").into(),
        name: manifest["name"].as_str().expect("a name").into(),
        workspace_id: WS,
        environment: Some("production"),
        manifest: Some(manifest),
        functions,
        approved_pipelines: BTreeSet::new(),
    };
    Inputs {
        level,
        workspace_filter: None,
        workspaces: vec![ws(WS, "main")],
        pipelines: vec![],
        apps: vec![app],
    }
}

#[test]
fn the_lease_desk_example_draws_its_writes_and_what_ships_to_airhouse() {
    let graph = assemble(&example_inputs(Level::Schema));
    let store = "oltp:app_lease_desk".to_string();
    let shipped = format!("schema:{WS}:oltp_lease_desk");
    assert_eq!(
        graph.nodes.keys().cloned().collect::<BTreeSet<_>>(),
        BTreeSet::from([app_id(), fn_id("leases"), store.clone(), shipped.clone()]),
    );
    assert_eq!(
        edge(&graph, &app_id(), &store, EdgeKind::Writes).via,
        set(&["migrations"])
    );
    assert_eq!(
        edge(&graph, &fn_id("leases"), &store, EdgeKind::Writes).via,
        set(&["oltp"])
    );
    let ships = edge(&graph, &store, &shipped, EdgeKind::Ships);
    assert_eq!(ships.tables, set(&["leases"]));
    assert_eq!(ships.excluded, set(&["leases.tenant_email"]));

    let graph = assemble(&example_inputs(Level::Table));
    let changes = format!("table:{WS}:oltp_lease_desk.leases__changes");
    let view = format!("table:{WS}:oltp_lease_desk.leases");
    let ships = edge(
        &graph,
        "oltp:app_lease_desk.leases",
        &changes,
        EdgeKind::Ships,
    );
    assert_eq!(ships.excluded, set(&["leases.tenant_email"]));
    edge(&graph, &changes, &view, EdgeKind::Reads);
}
