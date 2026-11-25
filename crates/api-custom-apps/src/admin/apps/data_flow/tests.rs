//! Edge assembly from fixture pipelines and manifests, and the shaping that
//! follows: fold, focus, depth and the node cap. No database.

use std::collections::BTreeSet;

use serde_json::{Value, json};
use uuid::Uuid;

use super::assemble::{AppIn, FunctionIn, Graph, Inputs, PipelineIn, WorkspaceIn, assemble};
use super::shape::shape;
use super::{DataFlowQuery, EdgeKind, FlowEdge, Level, NodeKind, Params};

const WS: Uuid = Uuid::from_u128(0x1111);
const OTHER_WS: Uuid = Uuid::from_u128(0x2222);
const APP: Uuid = Uuid::from_u128(0xaaaa);

fn ws(id: Uuid, name: &str) -> WorkspaceIn {
    WorkspaceIn {
        id,
        name: name.into(),
    }
}

/// A compiled `.airway.yml`, as `airway_pipelines.definition` holds it.
fn pipeline(
    workspace: Uuid,
    name: &str,
    kind: &str,
    dataset: &str,
    resources: &[&str],
) -> PipelineIn {
    PipelineIn {
        workspace_id: workspace,
        name: name.into(),
        file_path: format!("pipelines/{name}.airway.yml"),
        definition: json!({
            "name": name,
            "source": { "kind": kind, "config": { "api_key": "{{ secrets.KEY }}" } },
            "destination": { "database": "warehouse", "dataset_name": dataset },
            "resources": resources,
        }),
    }
}

/// The leases app: its own Airhouse and OLTP schemas, the change stream on
/// two tables, and four functions declaring a destination, a watch, the two
/// capabilities and two uploads.
fn leases_app() -> AppIn {
    AppIn {
        id: APP,
        slug: "lease-desk".into(),
        name: "Lease desk".into(),
        workspace_id: WS,
        environment: Some("production"),
        manifest: Some(json!({
            "schemaVersion": 2,
            "slug": "lease-desk",
            "migrations": { "dir": "migrations" },
            "airhouseMigrations": { "dir": "airhouse-migrations" },
            "oltpChanges": { "tables": { "leases": {}, "landlords": { "exclude": ["email"] } } },
        })),
        functions: vec![
            function(
                "export",
                json!({
                    "destinations": ["snowflake", "airhouse"],
                    "customerWarehouseWrites": { "snowflake": "finance reads it there" },
                }),
            ),
            function(
                "on-orders",
                json!({
                    "on": { "table": "warehouse:toast.orders" },
                    "airhouse": { "enabled": true },
                }),
            ),
            function(
                "record",
                json!({
                    "airhouse": { "enabled": true },
                    "oltp": { "enabled": true },
                }),
            ),
            function(
                "upload",
                json!({
                    "oxygen": { "sourceUploads": { "pipelines": ["toast_orders", "ubereats"] } },
                }),
            ),
        ],
        approved_pipelines: BTreeSet::from(["toast_orders".to_string()]),
    }
}

fn function(name: &str, spec: Value) -> FunctionIn {
    FunctionIn {
        name: name.into(),
        spec,
    }
}

fn inputs(level: Level) -> Inputs {
    Inputs {
        level,
        workspace_filter: None,
        workspaces: vec![ws(WS, "main"), ws(OTHER_WS, "lab")],
        pipelines: vec![
            pipeline(WS, "toast_orders", "toast", "toast", &["orders", "checks"]),
            pipeline(OTHER_WS, "lab_feed", "rest_api", "lab", &[]),
        ],
        apps: vec![leases_app()],
    }
}

fn edge<'g>(graph: &'g Graph, from: &str, to: &str, kind: EdgeKind) -> &'g FlowEdge {
    graph
        .edges
        .get(&(from.to_string(), to.to_string(), kind))
        .unwrap_or_else(|| {
            panic!(
                "no {kind:?} edge {from} -> {to}; edges: {:#?}",
                graph.edges.keys()
            )
        })
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn app_id() -> String {
    format!("app:{APP}")
}

fn fn_id(name: &str) -> String {
    format!("fn:{APP}:{name}")
}

#[test]
fn a_pipeline_reads_its_source_and_writes_its_dataset() {
    let graph = assemble(&inputs(Level::Schema));
    let pipe = format!("pipe:{WS}:toast_orders");
    let source = format!("source:{WS}:toast_orders");
    let schema = format!("schema:{WS}:warehouse:toast");

    assert_eq!(graph.nodes[&source].kind, NodeKind::Source);
    assert_eq!(graph.nodes[&source].label, "toast");
    let reads = edge(&graph, &source, &pipe, EdgeKind::Reads);
    assert!(reads.declared);
    assert_eq!(reads.via, set(&["airway"]));
    let writes = edge(&graph, &pipe, &schema, EdgeKind::Writes);
    assert_eq!(writes.tables, set(&["orders", "checks"]));

    let meta = &graph.nodes[&pipe].meta;
    assert_eq!(meta["source_kind"], "toast");
    assert_eq!(meta["dataset"], "toast");
    assert_eq!(meta["workspace"], "main");
    // Nothing of the source's config reaches the answer.
    assert!(
        !serde_json::to_string(&graph.nodes)
            .unwrap()
            .contains("secrets.KEY")
    );
}

#[test]
fn a_pipeline_with_no_resources_writes_its_whole_dataset() {
    let graph = assemble(&inputs(Level::Schema));
    let writes = edge(
        &graph,
        &format!("pipe:{OTHER_WS}:lab_feed"),
        &format!("schema:{OTHER_WS}:warehouse:lab"),
        EdgeKind::Writes,
    );
    assert!(writes.tables.is_empty());
}

#[test]
fn a_schema_separator_splits_each_resource_into_its_schema() {
    let mut separated = pipeline(WS, "ch", "clickhouse", "raw", &["sales___orders", "plain"]);
    separated.definition["destination"]["schema_separator"] = json!("___");
    let graph = assemble(&Inputs {
        pipelines: vec![separated],
        apps: vec![],
        ..inputs(Level::Schema)
    });
    let pipe = format!("pipe:{WS}:ch");
    assert_eq!(
        edge(
            &graph,
            &pipe,
            &format!("schema:{WS}:warehouse:sales"),
            EdgeKind::Writes
        )
        .tables,
        set(&["orders"])
    );
    assert_eq!(
        edge(
            &graph,
            &pipe,
            &format!("schema:{WS}:warehouse:raw"),
            EdgeKind::Writes
        )
        .tables,
        set(&["plain"])
    );
}

#[test]
fn one_schema_name_in_two_databases_is_two_nodes() {
    let mut in_snowflake = pipeline(WS, "sf_raw", "rest_api", "raw", &[]);
    in_snowflake.definition["destination"]["database"] = json!("snowflake");
    let in_airhouse = pipeline(WS, "ah_raw", "rest_api", "raw", &[]);
    let mut app = leases_app();
    app.functions = vec![
        function(
            "on-sf",
            json!({ "on": { "table": "snowflake:raw.orders" }, "airhouse": { "enabled": true } }),
        ),
        function(
            "on-own",
            json!({ "on": { "table": "raw.orders" }, "airhouse": { "enabled": true } }),
        ),
    ];
    let graph = assemble(&Inputs {
        pipelines: vec![in_snowflake, in_airhouse],
        apps: vec![app],
        ..inputs(Level::Schema)
    });
    let snowflake = format!("schema:{WS}:snowflake:raw");
    let warehouse = format!("schema:{WS}:warehouse:raw");
    let unnamed = format!("schema:{WS}:raw");
    edge(
        &graph,
        &format!("pipe:{WS}:sf_raw"),
        &snowflake,
        EdgeKind::Writes,
    );
    edge(
        &graph,
        &format!("pipe:{WS}:ah_raw"),
        &warehouse,
        EdgeKind::Writes,
    );
    assert_eq!(graph.nodes[&snowflake].meta["database"], "snowflake");
    assert_eq!(graph.nodes[&warehouse].meta["database"], "warehouse");
    // A watch that names the database joins that database's schema, and only it.
    edge(&graph, &snowflake, &fn_id("on-sf"), EdgeKind::Triggers);
    assert!(
        !graph
            .edges
            .contains_key(&(warehouse.clone(), fn_id("on-sf"), EdgeKind::Triggers))
    );
    // One that names none is the workspace's Airhouse: its own node, not a guess.
    edge(&graph, &unnamed, &fn_id("on-own"), EdgeKind::Triggers);
    assert!(graph.nodes[&unnamed].meta.get("database").is_none());
}

#[test]
fn a_definition_that_does_not_parse_is_a_marked_node_with_no_edges() {
    let mut broken = pipeline(WS, "broken", "toast", "toast", &[]);
    broken.definition = json!({ "name": "broken", "source": "not a mapping" });
    let graph = assemble(&Inputs {
        pipelines: vec![broken],
        apps: vec![],
        ..inputs(Level::Schema)
    });
    let node = &graph.nodes[&format!("pipe:{WS}:broken")];
    assert_eq!(node.meta["invalid"], true);
    assert!(graph.edges.is_empty());
}

#[test]
fn an_apps_manifest_declares_its_two_schemas_and_the_change_stream() {
    let graph = assemble(&inputs(Level::Schema));
    let airhouse = format!("schema:{WS}:app_lease_desk");
    let oltp = "oltp:app_lease_desk".to_string();
    let shipped = format!("schema:{WS}:oltp_lease_desk");

    assert_eq!(
        edge(&graph, &app_id(), &airhouse, EdgeKind::Writes).via,
        set(&["airhouseMigrations"])
    );
    assert_eq!(
        edge(&graph, &app_id(), &oltp, EdgeKind::Writes).via,
        set(&["migrations"])
    );
    let ships = edge(&graph, &oltp, &shipped, EdgeKind::Ships);
    assert_eq!(ships.tables, set(&["leases", "landlords"]));
    assert_eq!(ships.via, set(&["oltpChanges"]));
    assert_eq!(graph.nodes[&oltp].meta["plane"], "oltp");
    assert_eq!(graph.nodes[&app_id()].meta["environment"], "production");
}

#[test]
fn at_table_level_each_named_table_is_its_own_node() {
    let graph = assemble(&inputs(Level::Table));
    let from = "oltp:app_lease_desk.leases".to_string();
    let to = format!("table:{WS}:oltp_lease_desk.leases");
    let ships = edge(&graph, &from, &to, EdgeKind::Ships);
    assert!(ships.tables.is_empty());
    assert_eq!(graph.nodes[&to].kind, NodeKind::Table);
    assert_eq!(graph.nodes[&to].label, "oltp_lease_desk.leases");
    // A pipeline's resources split the same way; a schema named whole stays one.
    let pipe = format!("pipe:{WS}:toast_orders");
    edge(
        &graph,
        &pipe,
        &format!("table:{WS}:warehouse:toast.orders"),
        EdgeKind::Writes,
    );
    edge(
        &graph,
        &app_id(),
        &format!("schema:{WS}:app_lease_desk"),
        EdgeKind::Writes,
    );
}

#[test]
fn functions_declare_destinations_watches_capabilities_and_uploads() {
    let graph = assemble(&inputs(Level::Schema));
    let snowflake = format!("store:{WS}:snowflake");

    edge(&graph, &fn_id("export"), &snowflake, EdgeKind::Writes);
    assert_eq!(graph.nodes[&snowflake].meta["customer_warehouse"], true);
    assert_eq!(
        graph.nodes[&snowflake].meta["reason"],
        "finance reads it there"
    );
    assert!(
        graph.nodes[&format!("store:{WS}:airhouse")]
            .meta
            .get("customer_warehouse")
            .is_none()
    );

    let watched = edge(
        &graph,
        &format!("schema:{WS}:warehouse:toast"),
        &fn_id("on-orders"),
        EdgeKind::Triggers,
    );
    assert_eq!(watched.tables, set(&["orders"]));
    assert_eq!(watched.via, set(&["on"]));

    edge(
        &graph,
        &fn_id("record"),
        &format!("schema:{WS}:app_lease_desk"),
        EdgeKind::Writes,
    );
    edge(
        &graph,
        &fn_id("record"),
        "oltp:app_lease_desk",
        EdgeKind::Writes,
    );

    let approved = edge(
        &graph,
        &fn_id("upload"),
        &format!("pipe:{WS}:toast_orders"),
        EdgeKind::Triggers,
    );
    assert_eq!(approved.approved, Some(true));
    // Named, but compiled nowhere in the org: drawn as missing, and unapproved.
    let missing_id = format!("pipe:{WS}:ubereats");
    assert_eq!(
        edge(&graph, &fn_id("upload"), &missing_id, EdgeKind::Triggers).approved,
        Some(false)
    );
    assert_eq!(graph.nodes[&missing_id].meta["missing"], true);
    assert_eq!(
        graph.nodes[&fn_id("upload")].parent.as_deref(),
        Some(app_id().as_str())
    );
}

#[test]
fn an_upload_finds_its_pipeline_in_another_workspace_and_flags_a_name_held_twice() {
    let mut app = leases_app();
    app.functions = vec![function(
        "feed",
        json!({
            "oxygen": { "sourceUploads": { "pipelines": ["lab_feed", "twice"] } },
        }),
    )];
    let graph = assemble(&Inputs {
        pipelines: vec![
            pipeline(OTHER_WS, "lab_feed", "rest_api", "lab", &[]),
            pipeline(WS, "twice", "toast", "a", &[]),
            pipeline(OTHER_WS, "twice", "toast", "b", &[]),
        ],
        apps: vec![app],
        ..inputs(Level::Schema)
    });
    edge(
        &graph,
        &fn_id("feed"),
        &format!("pipe:{OTHER_WS}:lab_feed"),
        EdgeKind::Triggers,
    );
    for workspace in [WS, OTHER_WS] {
        let id = format!("pipe:{workspace}:twice");
        edge(&graph, &fn_id("feed"), &id, EdgeKind::Triggers);
        assert_eq!(graph.nodes[&id].meta["ambiguous"], true);
    }
}

#[test]
fn a_workspace_filter_keeps_other_pipelines_out_unless_an_app_names_one() {
    let graph = assemble(&Inputs {
        workspace_filter: Some(WS),
        ..inputs(Level::Schema)
    });
    assert!(graph.nodes.contains_key(&format!("pipe:{WS}:toast_orders")));
    assert!(
        !graph
            .nodes
            .contains_key(&format!("pipe:{OTHER_WS}:lab_feed"))
    );
    assert!(
        !graph
            .nodes
            .contains_key(&format!("source:{OTHER_WS}:lab_feed"))
    );
}

#[test]
fn a_block_the_platform_would_refuse_draws_nothing() {
    let mut app = leases_app();
    app.manifest = Some(json!({
        "migrations": null,
        // Without `migrations`, promote refuses this block; so does the view.
        "oltpChanges": { "tables": { "leases": {} } },
    }));
    app.functions = vec![function(
        "bad-watch",
        json!({ "on": { "table": "no_schema" } }),
    )];
    let graph = assemble(&Inputs {
        pipelines: vec![],
        apps: vec![app],
        ..inputs(Level::Schema)
    });
    assert!(graph.edges.is_empty(), "{:#?}", graph.edges.keys());
}

#[test]
fn an_app_whose_slug_names_no_writer_still_draws_its_functions_destinations() {
    let mut app = leases_app();
    app.slug = "has_underscore".into();
    let graph = assemble(&Inputs {
        apps: vec![app],
        ..inputs(Level::Schema)
    });
    assert!(!graph.nodes.contains_key("oltp:app_has_underscore"));
    edge(
        &graph,
        &fn_id("export"),
        &format!("store:{WS}:snowflake"),
        EdgeKind::Writes,
    );
}

// ── shape ───────────────────────────────────────────────────────────────────

#[test]
fn unfocused_functions_fold_into_their_app_and_keep_their_names() {
    let shaped = shape(assemble(&inputs(Level::Schema)), None, 2, 500).unwrap();
    assert!(!shaped.truncated);
    assert!(shaped.nodes.iter().all(|n| n.kind != NodeKind::Function));
    let find = |from: &str, to: &str, kind| {
        shaped
            .edges
            .iter()
            .find(|e| e.from == from && e.to == to && e.kind == kind)
            .unwrap_or_else(|| panic!("no {kind:?} {from} -> {to}"))
    };
    // The app's own declaration and two functions' capability, one edge.
    let own = find(
        &app_id(),
        &format!("schema:{WS}:app_lease_desk"),
        EdgeKind::Writes,
    );
    assert_eq!(own.via, set(&["airhouseMigrations", "airhouse"]));
    assert_eq!(own.from_functions, set(&["on-orders", "record"]));
    let watch = find(
        &format!("schema:{WS}:warehouse:toast"),
        &app_id(),
        EdgeKind::Triggers,
    );
    assert_eq!(watch.to_functions, set(&["on-orders"]));
    assert!(watch.from_functions.is_empty());
    // Folding never invents an endpoint.
    let ids: BTreeSet<&str> = shaped.nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(
        shaped
            .edges
            .iter()
            .all(|e| ids.contains(e.from.as_str()) && ids.contains(e.to.as_str()))
    );
}

#[test]
fn a_folded_upload_is_approved_only_when_every_one_is() {
    let mut app = leases_app();
    app.functions.push(function(
        "upload-2",
        json!({
            "oxygen": { "sourceUploads": { "pipelines": ["toast_orders"] } },
        }),
    ));
    app.approved_pipelines.clear();
    let graph = assemble(&Inputs {
        apps: vec![app],
        ..inputs(Level::Schema)
    });
    let shaped = shape(graph, None, 2, 500).unwrap();
    let upload = shaped
        .edges
        .iter()
        .find(|e| e.from == app_id() && e.to == format!("pipe:{WS}:toast_orders"))
        .unwrap();
    assert_eq!(upload.approved, Some(false));
    assert_eq!(upload.from_functions, set(&["upload", "upload-2"]));
}

#[test]
fn focusing_an_app_unfolds_its_functions_within_depth() {
    let shaped = shape(assemble(&inputs(Level::Schema)), Some(&app_id()), 1, 500).unwrap();
    let ids: BTreeSet<&str> = shaped.nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(ids.contains(fn_id("export").as_str()), "{ids:?}");
    assert!(ids.contains(format!("schema:{WS}:app_lease_desk").as_str()));
    // Two hops away (app → function → its destination): outside depth 1.
    assert!(!ids.contains(format!("store:{WS}:snowflake").as_str()));
    // The other workspace's pipeline reaches no app at all.
    assert!(!ids.contains(format!("pipe:{OTHER_WS}:lab_feed").as_str()));

    let deeper = shape(assemble(&inputs(Level::Schema)), Some(&app_id()), 3, 500).unwrap();
    let ids: BTreeSet<&str> = deeper.nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(ids.contains(format!("store:{WS}:snowflake").as_str()));
    assert!(ids.contains(format!("source:{WS}:toast_orders").as_str()));
}

#[test]
fn focusing_a_function_unfolds_its_app() {
    let focus = fn_id("upload");
    let shaped = shape(assemble(&inputs(Level::Schema)), Some(&focus), 1, 500).unwrap();
    let ids: BTreeSet<&str> = shaped.nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(ids.contains(app_id().as_str()));
    assert!(ids.contains(format!("pipe:{WS}:toast_orders").as_str()));
}

#[test]
fn a_focus_that_names_no_node_is_none() {
    assert!(shape(assemble(&inputs(Level::Schema)), Some("app:nope"), 2, 500).is_none());
}

#[test]
fn past_the_cap_the_graph_is_cut_and_says_so() {
    let shaped = shape(assemble(&inputs(Level::Schema)), None, 2, 3).unwrap();
    assert!(shaped.truncated);
    assert_eq!(shaped.nodes.len(), 3);
    // The cut keeps the apps first, and no edge dangles.
    assert!(shaped.nodes.iter().any(|n| n.id == app_id()));
    let ids: BTreeSet<&str> = shaped.nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(
        shaped
            .edges
            .iter()
            .all(|e| ids.contains(e.from.as_str()) && ids.contains(e.to.as_str()))
    );
}

#[test]
fn the_answer_is_the_same_every_time() {
    let a = shape(assemble(&inputs(Level::Schema)), None, 2, 500).unwrap();
    let b = shape(assemble(&inputs(Level::Schema)), None, 2, 500).unwrap();
    assert_eq!(a.nodes, b.nodes);
    assert_eq!(a.edges, b.edges);
}

#[test]
fn nodes_and_edges_serialize_in_the_documented_shape() {
    let shaped = shape(assemble(&inputs(Level::Schema)), None, 2, 500).unwrap();
    let node = serde_json::to_value(&shaped.nodes[0]).unwrap();
    assert!(
        node.get("type").is_some() && node.get("kind").is_none(),
        "{node}"
    );
    let edge = serde_json::to_value(&shaped.edges[0]).unwrap();
    assert_eq!(edge["declared"], true);
    assert!(["reads", "writes", "triggers", "ships"].contains(&edge["kind"].as_str().unwrap()));
}

// ── params ──────────────────────────────────────────────────────────────────

fn query(depth: Option<u32>, level: Option<&str>) -> DataFlowQuery {
    DataFlowQuery {
        org_id: Some(Uuid::from_u128(1)),
        workspace_id: None,
        focus: Some("  ".into()),
        depth,
        level: level.map(str::to_string),
    }
}

#[test]
fn params_default_and_refuse_out_of_range() {
    let params = Params::parse(query(None, None)).unwrap();
    assert_eq!(
        (params.depth, params.level, params.focus),
        (2, Level::Schema, None)
    );
    assert_eq!(
        Params::parse(query(Some(6), Some("table"))).unwrap().level,
        Level::Table
    );
    for bad in [
        query(Some(0), None),
        query(Some(7), None),
        query(None, Some("column")),
    ] {
        assert_eq!(
            Params::parse(bad).unwrap_err().status,
            axum::http::StatusCode::BAD_REQUEST
        );
    }
    let no_org = DataFlowQuery::default();
    assert_eq!(Params::parse(no_org).unwrap_err().code, "missing_org_id");
}
