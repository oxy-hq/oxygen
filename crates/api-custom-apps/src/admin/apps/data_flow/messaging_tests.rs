//! App-to-app edges from fixture manifests: calls, events, the refusals each is
//! marked with, the workspace filter, and folding onto apps. No database.

use std::collections::BTreeSet;

use serde_json::{Value, json};
use uuid::Uuid;

use super::super::assemble::{AppIn, FunctionIn, Inputs, WorkspaceIn, assemble};
use super::super::shape::shape;
use super::super::{EdgeKind, FlowEdge, Level, NodeKind};

const WS: Uuid = Uuid::from_u128(0x1111);
const OTHER_WS: Uuid = Uuid::from_u128(0x2222);
const BILLING: Uuid = Uuid::from_u128(0xb111);
const SHOP: Uuid = Uuid::from_u128(0x5409);
const LAB: Uuid = Uuid::from_u128(0x1ab0);

fn app(
    id: Uuid,
    slug: &str,
    workspace: Uuid,
    manifest: Value,
    functions: &[(&str, Value)],
) -> AppIn {
    AppIn {
        id,
        slug: slug.into(),
        name: slug.into(),
        workspace_id: workspace,
        environment: Some("production"),
        manifest: Some(manifest),
        functions: functions
            .iter()
            .map(|(name, spec)| FunctionIn {
                name: (*name).into(),
                spec: spec.clone(),
            })
            .collect(),
        approved_pipelines: BTreeSet::new(),
    }
}

/// `shop` calls billing's `charge` (callable), `refund` (not callable), `void`
/// (absent) and a `ghost` app; billing publishes `invoice.paid` from `charge`;
/// shop's `on-paid` listens to it, to `invoice.voided` (in the catalogue, but no
/// function may publish it) and to `invoice.lost` (not in the catalogue). `lab`,
/// in another workspace, listens to `invoice.paid` too and calls billing's
/// `charge`.
fn apps() -> Vec<AppIn> {
    vec![
        app(
            BILLING,
            "billing",
            WS,
            json!({
                "events": {
                    "invoice.paid": { "schema": { "type": "object" } },
                    "invoice.voided": { "schema": { "type": "object" } },
                },
            }),
            &[
                (
                    "charge",
                    json!({ "callable": true, "events": { "publish": ["invoice.paid"] } }),
                ),
                ("refund", json!({})),
            ],
        ),
        app(
            SHOP,
            "shop",
            WS,
            json!({ "calls": { "billing": ["charge", "refund", "void"], "ghost": ["run"] } }),
            &[(
                "on-paid",
                json!({ "on": ["billing/invoice.paid", "billing/invoice.voided", "billing/invoice.lost"] }),
            )],
        ),
        app(
            LAB,
            "lab",
            OTHER_WS,
            json!({ "calls": { "billing": ["charge"] } }),
            &[("tally", json!({ "on": "billing/invoice.paid" }))],
        ),
    ]
}

fn inputs(filter: Option<Uuid>) -> Inputs {
    Inputs {
        level: Level::Schema,
        workspace_filter: filter,
        workspaces: vec![
            WorkspaceIn {
                id: WS,
                name: "main".into(),
            },
            WorkspaceIn {
                id: OTHER_WS,
                name: "lab".into(),
            },
        ],
        pipelines: Vec::new(),
        apps: apps(),
    }
}

fn app_node(id: Uuid) -> String {
    format!("app:{id}")
}

fn fn_node(app: Uuid, name: &str) -> String {
    format!("fn:{app}:{name}")
}

fn find<'a>(
    edges: impl IntoIterator<Item = &'a FlowEdge>,
    from: &str,
    to: &str,
    kind: EdgeKind,
) -> &'a FlowEdge {
    edges
        .into_iter()
        .find(|e| e.from == from && e.to == to && e.kind == kind)
        .unwrap_or_else(|| panic!("no {kind:?} edge {from} -> {to}"))
}

fn problems(edge: &FlowEdge) -> Vec<&str> {
    edge.problems.iter().map(String::as_str).collect()
}

#[test]
fn calls_draw_to_the_callee_function_marked_with_what_the_platform_refuses() {
    let graph = assemble(&inputs(None));
    let edges = graph.edges.values();
    let shop = app_node(SHOP);
    let charge = find(
        edges.clone(),
        &shop,
        &fn_node(BILLING, "charge"),
        EdgeKind::Calls,
    );
    assert!(charge.problems.is_empty());
    assert_eq!(charge.via, BTreeSet::from(["calls".to_string()]));
    let refund = find(
        edges.clone(),
        &shop,
        &fn_node(BILLING, "refund"),
        EdgeKind::Calls,
    );
    assert_eq!(problems(refund), ["not_callable"]);
    let void = find(
        edges.clone(),
        &shop,
        &fn_node(BILLING, "void"),
        EdgeKind::Calls,
    );
    assert_eq!(problems(void), ["function_not_found"]);
    let ghost = find(edges, &shop, "app:missing:ghost", EdgeKind::Calls);
    assert_eq!(problems(ghost), ["app_not_found"]);
    let missing = &graph.nodes["app:missing:ghost"];
    assert_eq!(missing.kind, NodeKind::App);
    assert_eq!(missing.meta["missing"], json!(true));
}

#[test]
fn events_draw_from_each_publishing_function_with_the_event_names() {
    let graph = assemble(&inputs(None));
    let charge = fn_node(BILLING, "charge");
    let paid = find(
        graph.edges.values(),
        &charge,
        &fn_node(SHOP, "on-paid"),
        EdgeKind::Publishes,
    );
    assert_eq!(paid.events, BTreeSet::from(["invoice.paid".to_string()]));
    assert!(paid.problems.is_empty());
    // Declared in the catalogue, published by nobody: drawn from the app, marked.
    let voided = find(
        graph.edges.values(),
        &app_node(BILLING),
        &fn_node(SHOP, "on-paid"),
        EdgeKind::Publishes,
    );
    assert!(voided.events.contains("invoice.voided"));
    assert!(voided.problems.contains("never_published"));
    // Not in the catalogue either: both refusals, on the same app-level edge.
    assert!(voided.events.contains("invoice.lost"));
    assert!(voided.problems.contains("not_in_catalog"));
    find(
        graph.edges.values(),
        &charge,
        &fn_node(LAB, "tally"),
        EdgeKind::Publishes,
    );
}

#[test]
fn a_workspace_filter_keeps_the_other_end_of_an_edge_it_draws() {
    // Only `lab`'s workspace: its subscription still reaches billing, in `main`.
    let graph = assemble(&inputs(Some(OTHER_WS)));
    find(
        graph.edges.values(),
        &fn_node(BILLING, "charge"),
        &fn_node(LAB, "tally"),
        EdgeKind::Publishes,
    );
    assert_eq!(
        graph.nodes[&app_node(BILLING)].meta["slug"],
        json!("billing")
    );
    // Lab's own call into `main` is drawn; shop's call, with both ends in
    // `main`, is not.
    find(
        graph.edges.values(),
        &app_node(LAB),
        &fn_node(BILLING, "charge"),
        EdgeKind::Calls,
    );
    assert!(
        graph
            .edges
            .values()
            .all(|e| e.kind != EdgeKind::Calls || e.from == app_node(LAB))
    );
}

#[test]
fn a_workspace_filter_keeps_the_edges_the_other_end_declares() {
    // Only `main`: lab (in `lab`) declares both edges, billing (in `main`) is
    // the callee and the publisher.
    let graph = assemble(&inputs(Some(WS)));
    find(
        graph.edges.values(),
        &fn_node(BILLING, "charge"),
        &fn_node(LAB, "tally"),
        EdgeKind::Publishes,
    );
    find(
        graph.edges.values(),
        &app_node(LAB),
        &fn_node(BILLING, "charge"),
        EdgeKind::Calls,
    );
    assert_eq!(graph.nodes[&app_node(LAB)].meta["slug"], json!("lab"));
}

#[test]
fn folded_onto_apps_the_calls_merge_and_keep_every_refusal() {
    let shaped = shape(assemble(&inputs(None)), None, 2, 500).expect("no focus");
    let calls = find(
        &shaped.edges,
        &app_node(SHOP),
        &app_node(BILLING),
        EdgeKind::Calls,
    );
    // The caller is an app, so only the callee's side names functions.
    assert!(calls.from_functions.is_empty());
    assert_eq!(
        calls.to_functions,
        BTreeSet::from(["charge", "refund", "void"].map(String::from))
    );
    assert_eq!(problems(calls), ["function_not_found", "not_callable"]);
    let events = find(
        &shaped.edges,
        &app_node(BILLING),
        &app_node(SHOP),
        EdgeKind::Publishes,
    );
    assert_eq!(events.events.len(), 3);
    // Publishers and subscribers stay on their own side once folded.
    assert_eq!(
        events.from_functions,
        BTreeSet::from(["charge".to_string()])
    );
    assert_eq!(events.to_functions, BTreeSet::from(["on-paid".to_string()]));
}
