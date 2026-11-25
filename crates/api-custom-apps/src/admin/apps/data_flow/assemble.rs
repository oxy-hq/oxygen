//! The declared graph, from the rows [`super::load`] read. No I/O, so the
//! fixture tests drive it with hand-written pipelines and manifests.
//!
//! Every reader here is the one the platform already acts on: a pipeline is
//! parsed as [`AirwayPipelineSpec`] (what the worker runs), table watches by
//! `function_table_watches` (what the tracker reads), uploads by
//! `platform_access::function_specs` (what the approval page reads), and
//! `oltpChanges` by `oltp_changes_from_manifest` (what promote installs). A
//! block one of them refuses draws nothing, rather than a guess.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use agentic_airway::{AirwayPipelineSpec, DestinationSpec};
use oxy_custom_apps::events::table::function_table_watches;
use oxy_custom_apps::manifest::oltp_changes_from_manifest;
use oxy_custom_apps::platform_access::function_specs;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::{EdgeKind, FlowEdge, FlowNode, Level, NodeKind};
use crate::oltp_changes::relay::render::schema_name as shipped_schema;

/// What one read of the org hands the assembler.
pub(crate) struct Inputs {
    pub level: Level,
    /// Only this workspace's pipelines and apps get nodes of their own; a
    /// pipeline elsewhere in the org still appears when an app here names it.
    pub workspace_filter: Option<Uuid>,
    pub workspaces: Vec<WorkspaceIn>,
    /// Every compiled pipeline of the org, under each workspace's promoted
    /// revision. All of them, whatever the filter: an upload names a pipeline,
    /// never a workspace, so finding it takes the whole org.
    pub pipelines: Vec<PipelineIn>,
    pub apps: Vec<AppIn>,
}

pub(crate) struct WorkspaceIn {
    pub id: Uuid,
    pub name: String,
}

pub(crate) struct PipelineIn {
    pub workspace_id: Uuid,
    pub name: String,
    pub file_path: String,
    /// The `.airway.yml` as JSON (`airway_pipelines.definition`).
    pub definition: Value,
}

pub(crate) struct AppIn {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub workspace_id: Uuid,
    /// Whose build `manifest` and `functions` are: `production`, else
    /// `staging`. `None` for an app with no build yet.
    pub environment: Option<&'static str>,
    /// `oxy-app.json` as the build recorded it (or the app's override).
    pub manifest: Option<Value>,
    pub functions: Vec<FunctionIn>,
    /// Pipelines an org admin approved this app to upload into.
    pub approved_pipelines: BTreeSet<String>,
}

/// One function of the live build: its entry in `oxy-app.json`.
pub(crate) struct FunctionIn {
    pub name: String,
    pub spec: Value,
}

/// Nodes and edges keyed for merging; `BTreeMap`s, so every answer comes out
/// in the same order.
#[derive(Debug, Default)]
pub(crate) struct Graph {
    pub nodes: BTreeMap<String, FlowNode>,
    pub edges: BTreeMap<(String, String, EdgeKind), FlowEdge>,
}

impl Graph {
    /// The node `id`, created as `kind`/`label` when new.
    pub(crate) fn node(&mut self, id: &str, kind: NodeKind, label: &str) -> &mut FlowNode {
        self.nodes
            .entry(id.to_string())
            .or_insert_with(|| FlowNode {
                id: id.to_string(),
                kind,
                label: label.to_string(),
                parent: None,
                meta: Map::new(),
            })
    }

    /// The edge `from → to` of `kind`, created when new, with `via` recorded.
    pub(crate) fn edge(
        &mut self,
        from: &str,
        to: &str,
        kind: EdgeKind,
        via: &str,
    ) -> &mut FlowEdge {
        let edge = self
            .edges
            .entry((from.to_string(), to.to_string(), kind))
            .or_insert_with(|| FlowEdge {
                from: from.to_string(),
                to: to.to_string(),
                kind,
                declared: true,
                via: BTreeSet::new(),
                tables: BTreeSet::new(),
                functions: BTreeSet::new(),
                approved: None,
            });
        edge.via.insert(via.to_string());
        edge
    }
}

pub(crate) fn app_node_id(app: Uuid) -> String {
    format!("app:{app}")
}

pub(crate) fn function_node_id(app: Uuid, name: &str) -> String {
    format!("fn:{app}:{name}")
}

pub(crate) fn pipeline_node_id(workspace: Uuid, name: &str) -> String {
    format!("pipe:{workspace}:{name}")
}

/// A schema an edge reaches, before the level decides its node.
#[derive(Clone, Copy)]
enum Place<'a> {
    /// A schema of one of a workspace's databases. `database` is the name
    /// `config.yml` gives it (a pipeline's `database:`, a watch's
    /// `<database>:` prefix), and part of the node's identity: two databases
    /// can each hold a schema by one name, and they are not one schema. `None`
    /// is the workspace's own Airhouse as the platform reaches it without a
    /// name: an app's `app_<writer>` and `oltp_<writer>`, a watch with no
    /// prefix. A schema named both ways is drawn twice rather than merged
    /// with a stranger.
    Warehouse {
        workspace: Uuid,
        database: Option<&'a str>,
        schema: &'a str,
    },
    /// A schema of the org's OLTP store.
    Oltp { schema: &'a str },
}

impl Place<'_> {
    fn schema(&self) -> &str {
        match self {
            Place::Warehouse { schema, .. } | Place::Oltp { schema } => schema,
        }
    }

    /// `schema:<ws>[:<database>]:<schema>` or `oltp:<schema>`.
    fn schema_id(&self) -> String {
        match self {
            Place::Warehouse {
                workspace,
                database: Some(db),
                schema,
            } => {
                format!("schema:{workspace}:{db}:{schema}")
            }
            Place::Warehouse {
                workspace,
                database: None,
                schema,
            } => {
                format!("schema:{workspace}:{schema}")
            }
            Place::Oltp { schema } => format!("oltp:{schema}"),
        }
    }

    /// `table:<ws>[:<database>]:<schema>.<table>` or `oltp:<schema>.<table>`.
    fn table_id(&self, table: &str) -> String {
        match self {
            Place::Warehouse {
                workspace,
                database: Some(db),
                schema,
            } => {
                format!("table:{workspace}:{db}:{schema}.{table}")
            }
            Place::Warehouse {
                workspace,
                database: None,
                schema,
            } => {
                format!("table:{workspace}:{schema}.{table}")
            }
            Place::Oltp { schema } => format!("oltp:{schema}.{table}"),
        }
    }
}

/// The declared graph of `inputs`, functions unfolded.
pub(crate) fn assemble(inputs: &Inputs) -> Graph {
    let mut builder = Builder::new(inputs);
    let wanted = |workspace: Uuid| inputs.workspace_filter.is_none_or(|w| w == workspace);
    for pipeline in inputs.pipelines.iter().filter(|p| wanted(p.workspace_id)) {
        builder.add_pipeline(pipeline);
    }
    for app in inputs.apps.iter().filter(|a| wanted(a.workspace_id)) {
        builder.add_app(app);
    }
    builder.graph
}

struct Builder<'a> {
    graph: Graph,
    level: Level,
    workspace_names: HashMap<Uuid, &'a str>,
    /// Pipeline name → the workspaces holding one by that name.
    pipelines_by_name: BTreeMap<&'a str, Vec<Uuid>>,
}

impl<'a> Builder<'a> {
    fn new(inputs: &'a Inputs) -> Self {
        let mut pipelines_by_name: BTreeMap<&str, Vec<Uuid>> = BTreeMap::new();
        for p in &inputs.pipelines {
            pipelines_by_name
                .entry(p.name.as_str())
                .or_default()
                .push(p.workspace_id);
        }
        Self {
            graph: Graph::default(),
            level: inputs.level,
            workspace_names: inputs
                .workspaces
                .iter()
                .map(|w| (w.id, w.name.as_str()))
                .collect(),
            pipelines_by_name,
        }
    }

    fn workspace_meta(&self, workspace: Uuid) -> Map<String, Value> {
        let mut meta = Map::new();
        meta.insert("workspace_id".into(), json!(workspace));
        if let Some(name) = self.workspace_names.get(&workspace) {
            meta.insert("workspace".into(), json!(name));
        }
        meta
    }

    /// The node for `place` — or, at table level, for `table` in it — created
    /// when new. Returns its id, and the table the edge should record when the
    /// node is the whole schema.
    fn place(&mut self, place: Place<'_>, table: Option<&str>) -> (String, Option<String>) {
        let mut meta = self.place_meta(place);
        let schema = place.schema();
        let split = self.level == Level::Table && table.is_some();
        let (id, kind, label) = match table.filter(|_| split) {
            Some(t) => {
                meta.insert("schema".into(), json!(schema));
                (place.table_id(t), NodeKind::Table, format!("{schema}.{t}"))
            }
            None => (place.schema_id(), NodeKind::Schema, schema.to_string()),
        };
        let node = self.graph.node(&id, kind, &label);
        node.meta.extend(meta);
        let recorded = if split {
            None
        } else {
            table.map(str::to_string)
        };
        (id, recorded)
    }

    /// Where a place lives: its plane, and for a warehouse its workspace and
    /// the database it is named in.
    fn place_meta(&self, place: Place<'_>) -> Map<String, Value> {
        match place {
            Place::Warehouse {
                workspace,
                database,
                ..
            } => {
                let mut meta = self.workspace_meta(workspace);
                meta.insert("plane".into(), json!("warehouse"));
                if let Some(database) = database {
                    meta.insert("database".into(), json!(database));
                }
                meta
            }
            Place::Oltp { .. } => {
                let mut meta = Map::new();
                meta.insert("plane".into(), json!("oltp"));
                meta
            }
        }
    }

    fn link(
        &mut self,
        from: &str,
        to: &str,
        kind: EdgeKind,
        via: &str,
        table: Option<String>,
    ) -> &mut FlowEdge {
        let edge = self.graph.edge(from, to, kind, via);
        edge.tables.extend(table);
        edge
    }

    // ── pipelines ───────────────────────────────────────────────────────────

    fn add_pipeline(&mut self, p: &PipelineIn) {
        let id = pipeline_node_id(p.workspace_id, &p.name);
        let mut meta = self.workspace_meta(p.workspace_id);
        meta.insert("file_path".into(), json!(p.file_path));
        let spec = serde_json::from_value::<AirwayPipelineSpec>(p.definition.clone()).ok();
        if spec.is_none() {
            // Not the parse error: it can quote the definition's values.
            meta.insert("invalid".into(), json!(true));
        }
        self.graph
            .node(&id, NodeKind::Pipeline, &p.name)
            .meta
            .extend(meta);
        if let Some(spec) = spec {
            self.add_pipeline_spec(p, &id, &spec);
        }
    }

    fn add_pipeline_spec(&mut self, p: &PipelineIn, id: &str, spec: &AirwayPipelineSpec) {
        let mut meta = Map::new();
        meta.insert("source_kind".into(), json!(spec.source.kind));
        if let Some(description) = &spec.description {
            meta.insert("description".into(), json!(description));
        }
        let source = format!("source:{}:{}", p.workspace_id, p.name);
        let mut source_meta = self.workspace_meta(p.workspace_id);
        source_meta.insert("kind".into(), json!(spec.source.kind));
        self.graph
            .node(&source, NodeKind::Source, &spec.source.kind)
            .meta
            .extend(source_meta);
        self.graph.edge(&source, id, EdgeKind::Reads, "airway");
        match &spec.destination {
            DestinationSpec::Reference(r) => {
                meta.insert("database".into(), json!(r.database));
                meta.insert("dataset".into(), json!(r.dataset_name));
                self.add_landing(p.workspace_id, id, spec, r);
            }
            DestinationSpec::Inline(c) => {
                let store = format!("store:{}:{}", p.workspace_id, c.kind);
                let store_meta = self.workspace_meta(p.workspace_id);
                self.graph
                    .node(&store, NodeKind::Store, &c.kind)
                    .meta
                    .extend(store_meta);
                self.graph.edge(id, &store, EdgeKind::Writes, "airway");
            }
        }
        if let Some(node) = self.graph.nodes.get_mut(id) {
            node.meta.extend(meta);
        }
    }

    /// The schema(s) a `database:` destination writes: `dataset_name`, or —
    /// with a `schema_separator` — the schema each resource's name carries.
    /// The resources name the tables (nested child tables are not known
    /// before a run).
    fn add_landing(
        &mut self,
        workspace: Uuid,
        id: &str,
        spec: &AirwayPipelineSpec,
        r: &agentic_airway::DestinationRef,
    ) {
        let landings: Vec<(&str, Option<&str>)> = if spec.resources.is_empty() {
            vec![(r.dataset_name.as_str(), None)]
        } else {
            spec.resources
                .iter()
                .map(|resource| {
                    let (schema, table) =
                        landing(&r.dataset_name, r.schema_separator.as_deref(), resource);
                    (schema, Some(table))
                })
                .collect()
        };
        for (schema, table) in landings {
            let place = Place::Warehouse {
                workspace,
                database: Some(&r.database),
                schema,
            };
            let (to, recorded) = self.place(place, table);
            self.link(id, &to, EdgeKind::Writes, "airway", recorded);
        }
    }

    // ── apps ────────────────────────────────────────────────────────────────

    fn add_app(&mut self, app: &AppIn) {
        let id = app_node_id(app.id);
        let mut meta = self.workspace_meta(app.workspace_id);
        meta.insert("slug".into(), json!(app.slug));
        meta.insert("environment".into(), json!(app.environment));
        self.graph
            .node(&id, NodeKind::App, &app.name)
            .meta
            .extend(meta);
        let writer = oxy_oltp::schema::app_writer_name(&app.slug);
        if let (Some(manifest), Some(writer)) = (&app.manifest, writer.as_deref()) {
            self.add_app_blocks(app, &id, manifest, writer);
        }
        for function in &app.functions {
            self.add_function(app, writer.as_deref(), function);
        }
    }

    /// The app-level declarations: its two schemas and the change stream.
    fn add_app_blocks(&mut self, app: &AppIn, id: &str, manifest: &Value, writer: &str) {
        let own = own_schema(writer);
        let warehouse = Place::Warehouse {
            workspace: app.workspace_id,
            database: None,
            schema: &own,
        };
        let oltp = Place::Oltp { schema: &own };
        if declares(manifest, "airhouseMigrations") {
            let (to, _) = self.place(warehouse, None);
            self.graph
                .edge(id, &to, EdgeKind::Writes, "airhouseMigrations");
        }
        if declares(manifest, "migrations") {
            let (to, _) = self.place(oltp, None);
            self.graph.edge(id, &to, EdgeKind::Writes, "migrations");
        }
        let Ok(Some(changes)) = oltp_changes_from_manifest(manifest) else {
            return;
        };
        let shipped = shipped_schema(writer);
        let into = Place::Warehouse {
            workspace: app.workspace_id,
            database: None,
            schema: &shipped,
        };
        for table in changes.tables.keys() {
            let (from, recorded) = self.place(oltp, Some(table));
            let (to, _) = self.place(into, Some(table));
            self.link(&from, &to, EdgeKind::Ships, "oltpChanges", recorded);
        }
    }

    fn add_function(&mut self, app: &AppIn, writer: Option<&str>, f: &FunctionIn) {
        let id = function_node_id(app.id, &f.name);
        self.graph.node(&id, NodeKind::Function, &f.name).parent = Some(app_node_id(app.id));
        self.add_destinations(app, &id, &f.spec);
        self.add_watches(app, &id, &f.spec);
        if let Some(writer) = writer {
            self.add_own_stores(app, &id, writer, &f.spec);
        }
        self.add_uploads(app, &id, &f.spec);
    }

    /// `destinations`: the databases `ctx.warehouse` may write. One named in
    /// `customerWarehouseWrites` is a customer warehouse, with its reason.
    fn add_destinations(&mut self, app: &AppIn, id: &str, spec: &Value) {
        let exceptions = spec
            .get("customerWarehouseWrites")
            .and_then(Value::as_object);
        for database in strings(spec.get("destinations")) {
            let store = format!("store:{}:{database}", app.workspace_id);
            let mut meta = self.workspace_meta(app.workspace_id);
            if let Some(reason) = exceptions.and_then(|e| e.get(database)) {
                meta.insert("customer_warehouse".into(), json!(true));
                meta.insert("reason".into(), reason.clone());
            }
            self.graph
                .node(&store, NodeKind::Store, database)
                .meta
                .extend(meta);
            self.graph
                .edge(id, &store, EdgeKind::Writes, "destinations");
        }
    }

    /// `on: { table }`: each watched table wakes the function.
    fn add_watches(&mut self, app: &AppIn, id: &str, spec: &Value) {
        let Ok(watches) = function_table_watches(spec) else {
            return;
        };
        for watch in watches {
            let place = Place::Warehouse {
                workspace: app.workspace_id,
                database: watch.database.as_deref(),
                schema: &watch.schema,
            };
            let (from, recorded) = self.place(place, Some(&watch.table));
            self.link(&from, id, EdgeKind::Triggers, "on", recorded);
        }
    }

    /// The `airhouse` and `oltp` capabilities: the function writes the app's
    /// own schema in each.
    fn add_own_stores(&mut self, app: &AppIn, id: &str, writer: &str, spec: &Value) {
        let own = own_schema(writer);
        if enabled(spec, "/airhouse/enabled") {
            let place = Place::Warehouse {
                workspace: app.workspace_id,
                database: None,
                schema: &own,
            };
            let (to, _) = self.place(place, None);
            self.graph.edge(id, &to, EdgeKind::Writes, "airhouse");
        }
        if enabled(spec, "/oltp/enabled") {
            let (to, _) = self.place(Place::Oltp { schema: &own }, None);
            self.graph.edge(id, &to, EdgeKind::Writes, "oltp");
        }
    }

    /// `oxygen.sourceUploads.pipelines`: the function feeds each pipeline's
    /// landing zone, once an org admin approves it.
    fn add_uploads(&mut self, app: &AppIn, id: &str, spec: &Value) {
        for name in function_specs(spec).pipelines() {
            let approved = app.approved_pipelines.contains(&name);
            for pipeline in self.resolve_pipeline(&name, app.workspace_id) {
                let edge =
                    self.graph
                        .edge(id, &pipeline, EdgeKind::Triggers, "oxygen.sourceUploads");
                edge.approved = Some(approved);
            }
        }
    }

    /// The pipeline node(s) an upload names, as `app_ops::resolve` finds them:
    /// by name across the org. None found is drawn as a missing pipeline in
    /// the app's workspace; more than one is marked ambiguous, which the
    /// upload refuses.
    fn resolve_pipeline(&mut self, name: &str, fallback: Uuid) -> Vec<String> {
        let holders = self
            .pipelines_by_name
            .get(name)
            .cloned()
            .unwrap_or_default();
        if holders.is_empty() {
            let id = pipeline_node_id(fallback, name);
            let mut meta = self.workspace_meta(fallback);
            meta.insert("missing".into(), json!(true));
            self.graph
                .node(&id, NodeKind::Pipeline, name)
                .meta
                .extend(meta);
            return vec![id];
        }
        let ambiguous = holders.len() > 1;
        holders
            .into_iter()
            .map(|workspace| {
                let id = pipeline_node_id(workspace, name);
                let meta = self.workspace_meta(workspace);
                let node = self.graph.node(&id, NodeKind::Pipeline, name);
                node.meta.extend(meta);
                if ambiguous {
                    node.meta.insert("ambiguous".into(), json!(true));
                }
                id
            })
            .collect()
    }
}

/// Where a resource lands: `dataset`/`resource`, or with a separator,
/// `<schema><sep><table>` split at its first separator.
fn landing<'r>(dataset: &'r str, separator: Option<&str>, resource: &'r str) -> (&'r str, &'r str) {
    separator
        .filter(|sep| !sep.is_empty())
        .and_then(|sep| resource.split_once(sep))
        .filter(|(schema, table)| !schema.is_empty() && !table.is_empty())
        .unwrap_or((dataset, resource))
}

/// `app_<writer>`, the app's own schema in Airhouse and in its OLTP store.
fn own_schema(writer: &str) -> String {
    oxy_oltp::schema::WriterRef::app(writer)
        .map(|w| w.schema_name())
        .unwrap_or_else(|_| format!("app_{writer}"))
}

/// The manifest declares `key`: present, and not an explicit `null`.
fn declares(manifest: &Value, key: &str) -> bool {
    manifest.get(key).is_some_and(|v| !v.is_null())
}

fn enabled(spec: &Value, pointer: &str) -> bool {
    spec.pointer(pointer).and_then(Value::as_bool) == Some(true)
}

fn strings(value: Option<&Value>) -> BTreeSet<&str> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .collect()
}
