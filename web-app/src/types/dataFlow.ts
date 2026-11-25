/**
 * `GET /api/customer-apps/data-flow` — one org's declared data flow: ingestion
 * sources, Airway pipelines, stores (Airhouse and OLTP schemas, named databases)
 * and the custom apps that write, ship and watch them. Mirrors
 * `crates/api-custom-apps/src/admin/apps/data_flow.rs`.
 */

export type DataFlowNodeType =
  | "source"
  | "pipeline"
  | "store"
  | "schema"
  | "table"
  | "app"
  | "function";

/** Which way the data moves along an edge, and who moves it. */
export type DataFlowEdgeKind = "reads" | "writes" | "triggers" | "ships";

export type DataFlowLevel = "schema" | "table";

export interface DataFlowNode {
  /** Stable: `pipe:<ws>:<name>`, `schema:<ws>[:<database>]:<schema>`, `app:<id>`,
   *  `fn:<app>:<name>`, … */
  id: string;
  type: DataFlowNodeType;
  label: string;
  /** A function's app. */
  parent?: string;
  /** Free-form facts: `workspace`, `plane` (`warehouse` | `oltp`), `source_kind`,
   *  `missing`, `ambiguous`, `invalid`, `customer_warehouse`, `environment`, … */
  meta: Record<string, unknown>;
}

export interface DataFlowEdge {
  from: string;
  to: string;
  kind: DataFlowEdgeKind;
  /** Always true until observed edges ship. */
  declared: boolean;
  /** What declared it: `airway`, `airhouseMigrations`, `migrations`, `oltpChanges`,
   *  `destinations`, `airhouse`, `oltp`, `on`, `oxygen.sourceUploads`. */
  via: string[];
  tables?: string[];
  /** The functions folded into an app-level edge. */
  functions?: string[];
  /** An upload into a pipeline: whether an org admin approved it. */
  approved?: boolean;
}

export interface DataFlow {
  generated_at: string;
  org: { id: string; name: string; slug: string };
  workspaces: { id: string; name: string }[];
  truncated: boolean;
  level: DataFlowLevel;
  nodes: DataFlowNode[];
  edges: DataFlowEdge[];
}

export interface DataFlowQuery {
  org_id: string;
  workspace_id?: string;
  focus?: string;
  depth?: number;
  level?: DataFlowLevel;
}
