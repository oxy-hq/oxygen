use oxy_airlayer_compat::schema::models::{AdditivityClass, MeasureType};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::ToSchema;

#[derive(Deserialize)]
pub struct WmInstancesQuery {
    pub entity: String,
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// `reach` — on the customer-app route, list only the instances whose
    /// place the viewer reaches, filtered inside the scan so the page is a
    /// page of the right set (`internal-docs/operating-graph.md` §3.6).
    /// Ignored on the workspace route, where the viewer is a builder.
    #[serde(default)]
    pub scope: Option<String>,
    /// The bundle's own app, so app-admin standing counts toward `scope`.
    /// Parsed leniently — see `app_id()` — so a malformed value narrows the
    /// reach rather than failing the listing.
    #[serde(default)]
    pub app: Option<String>,
}

impl WmInstancesQuery {
    pub fn app_id(&self) -> Option<uuid::Uuid> {
        self.app.as_deref().and_then(|a| a.parse().ok())
    }
}
fn default_limit() -> usize {
    50
}

#[derive(Serialize, Deserialize, Clone)]
pub struct WmInstanceItem {
    pub key: String,
    pub display: String,
    /// The org location this instance is, when the entity is bound to the
    /// registry and the key is mapped. Absent otherwise — an unmapped store
    /// is a fact the screen can show, not an error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<WmInstanceLocation>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct WmInstanceLocation {
    pub id: uuid::Uuid,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<uuid::Uuid>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct WmInstancesResponse {
    pub total: usize,
    /// True when there are more records beyond the returned page (total hit the limit).
    pub has_more: bool,
    pub items: Vec<WmInstanceItem>,
}

#[derive(Deserialize)]
pub struct WmFilterCountsRequest {
    pub entity_id: String,
    pub key_value: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct WmEntityCount {
    pub matched: u64,
    pub total: u64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct WmFilterCountsResponse {
    pub counts: HashMap<String, WmEntityCount>,
}

/// Individual event emitted by the streaming filter-counts SSE endpoint.
/// Each event carries either a `total` count, a `matched` count, or a
/// `done: true` sentinel that marks stream completion.  The frontend
/// merges events into a running `Record<entityName, WmEntityCount>`.
#[derive(Serialize)]
pub struct WmFilterCountEvent {
    pub entity_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched: Option<u64>,
    /// Sample of reachable descendant rows at this entity's grain (display strings).
    /// Empty for ancestors, the seed, and total-only events.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sample: Vec<String>,
    /// Navigation keys aligned with `sample` (see `sample_row_to_display_key`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sample_keys: Vec<String>,
    pub done: bool,
}

#[derive(Deserialize)]
pub struct WmInstanceDetailQuery {
    pub entity: String,
    pub key: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct WmAttrValue {
    pub name: String,
    pub value: String,
    /// Display label from `.world-model.yml`. Falls back to `name` in the UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct WmParentRef {
    pub promotion: String,
    pub key: String,
    pub display: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct WmChildSample {
    pub promotion: String,
    pub fiber_count: u64,
    /// Display labels for each sample row (human-readable, may join composite PK parts).
    pub sample: Vec<String>,
    /// Canonical navigation key for each sample row — passed back as `key` to the
    /// instance-detail endpoint.  Single-PK: plain value.  Composite-PK: JSON array
    /// of the individual column values (e.g. `["70978","177411"]`).
    pub sample_keys: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct WmComputedMeasure {
    pub name: String,
    pub measure_type: String,
    pub value: String,
    pub fiber_count: u64,
    /// Display label from `.world-model.yml`. Falls back to `name` in the UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct WmMeasureName {
    pub name: String,
    pub measure_type: String,
    /// Display label from `.world-model.yml`. Falls back to `name` in the UI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct WmInstanceDetailResponse {
    pub entity_id: String,
    pub key_value: String,
    pub display: String,
    pub attributes: Vec<WmAttrValue>,
    pub promotes_to: Vec<WmParentRef>,
    pub receives_from: Vec<WmChildSample>,
    pub computed_measures: Vec<WmComputedMeasure>,
}

/// Streaming event emitted by `GET /semantic/world-model/instance-detail`.
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WmInstanceDetailEvent {
    /// First event: entity attributes.  Emitted as soon as the attrs query returns.
    Init {
        entity_id: String,
        key_value: String,
        display: String,
        attributes: Vec<WmAttrValue>,
    },
    /// Parent promotion(s) — emitted after Phase 2 parent lookup.
    Parent { promotes_to: Vec<WmParentRef> },
    /// One child sample — streamed individually as each child query completes.
    Child { child: WmChildSample },
    /// Emitted immediately before any measure queries fire — lists all measure names and
    /// types derived from schema alone (no DB round-trip). Lets the frontend show skeletons.
    MeasureNames { measure_names: Vec<WmMeasureName> },
    /// One batch of computed measures — emitted once per completed query group
    /// (own measures = 1 event, each induced source-view group = 1 event).
    Measure {
        computed_measures: Vec<WmComputedMeasure>,
    },
    /// Terminal event.
    Done,
}

// ── World Model ───────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct WmMeasure {
    pub name: String,
    pub measure_type: MeasureType,
    pub additivity: AdditivityClass,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Raw SQL expression for the measure (e.g. "amount", "1" for count).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expr: Option<String>,
    /// True when this measure decomposes into component children in the metric
    /// tree (a composite `type: number` measure). Drives the driver-tree
    /// expand affordance — leaf measures have no breakdown.
    pub has_breakdown: bool,
    /// The measure's `default_cohort:` (`"entity.cohort_name"`), when it
    /// declares one. Schema-only — no query — and it is what lets the instance
    /// panel decide whether to offer "Compared to peers" at all: without it the
    /// only way to learn a measure declares no cohort is to spend the
    /// resolution (two entity-grain warehouse pulls) and be told so.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_cohort: Option<String>,
}

#[derive(Serialize)]
pub struct WmInducedMeasure {
    pub name: String,
    pub measure_type: MeasureType,
    pub additivity: AdditivityClass,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub promoted_from: String,
    /// Raw SQL expression of the source measure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expr: Option<String>,
    /// Ordered entity names walked from source grain to this entity's grain.
    pub path: Vec<String>,
    /// The SOURCE measure's `default_cohort:`, read off the literal view it is
    /// declared on — the same view this induced measure is addressed through
    /// (`view.measure` uses `promoted_from`), so the cohort endpoint resolves
    /// the same declaration the panel gated on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_cohort: Option<String>,
}

#[derive(Serialize)]
pub struct WmDimension {
    pub name: String,
    pub dim_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Serialize)]
pub struct WmEntity {
    pub id: String,
    pub label: String,
    pub view: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub depth: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_field: Option<String>,
    pub dimensions: Vec<WmDimension>,
    pub own_measures: Vec<WmMeasure>,
    pub induced_measures: Vec<WmInducedMeasure>,
}

#[derive(Serialize)]
pub struct WmEdge {
    pub from: String,
    pub to: String,
    pub functional: bool,
}

#[derive(Serialize)]
pub struct WorldModelResponse {
    pub entities: Vec<WmEntity>,
    pub edges: Vec<WmEdge>,
}

#[derive(Deserialize)]
pub struct WmFilterInstancesQuery {
    /// The active/selected instance's entity (the filter seed).
    pub seed_entity: String,
    /// The active instance's key.
    pub seed_key: String,
    /// Target entity whose reachable rows to list.
    pub entity: String,
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

#[derive(Debug, Deserialize)]
pub struct WmMeasureBreakdownQuery {
    pub entity: String,
    pub key: String,
    pub measure: String,
    #[serde(default)]
    pub datasource: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WmBreakdownNode {
    /// Metric node id `view.measure`.
    pub id: String,
    pub view: String,
    pub measure: String,
    pub label: String,
    pub measure_type: String,
    pub is_composite: bool,
    pub is_root: bool,
    pub expr: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WmBreakdownEdge {
    pub from: String,
    pub to: String,
    /// "add" | "sub" | "mul" | "div".
    pub operator: String,
    pub sign: f64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WmMeasureBreakdownEvent {
    Init {
        root: String,
        nodes: Vec<WmBreakdownNode>,
        edges: Vec<WmBreakdownEdge>,
    },
    Value {
        node_id: String,
        value: Option<String>,
        unvalued_reason: Option<String>,
    },
    Done,
}
