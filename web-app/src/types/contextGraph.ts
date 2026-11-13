export interface ContextGraphNode {
  id: string;
  type:
    | "table"
    | "view"
    | "topic"
    | "procedure"
    | "workflow"
    | "automation"
    | "sql_query"
    | "agent"
    | "entity";
  label: string;
  data: {
    name: string;
    path?: string;
    description?: string;
    database?: string;
    datasource?: string;
    metadata?: Record<string, unknown>;
  };
}

export interface ContextGraphEdge {
  id: string;
  source: string;
  target: string;
  label?: string;
  type?: "references" | "uses" | "contains";
}

export interface ContextGraph {
  nodes: ContextGraphNode[];
  edges: ContextGraphEdge[];
  /**
   * True when at least one database answered `datasets: null` — the instance
   * that served the request could not look, because the semantic sync directory
   * lives in the working copy and a stateless replica does not have one.
   *
   * It is NOT the same as "this workspace has no tables", and the graph cannot
   * tell the difference on its own: both produce zero table nodes. Carrying the
   * flag is what lets the overview say "not visible from here" instead of
   * quietly dropping the row and looking complete.
   */
  tablesUnknown: boolean;
}

export interface View {
  name: string;
  path: string;
  description?: string;
  datasource: string;
  table: string;
  entities?: Array<{
    name: string;
    type: string;
    description?: string;
    keys?: string[];
  }>;
  dimensions?: Array<{
    name: string;
    type: string;
    sql?: string;
  }>;
  measures?: Array<{
    name: string;
    type: string;
    sql?: string;
  }>;
}

export interface Topic {
  name: string;
  path: string;
  description?: string;
  views: string[];
  base_view?: string;
  default_filters?: Array<{
    field: string;
    op: string;
    value: unknown;
  }>;
}

/** One compiled artifact: its name, workspace-relative path and parsed YAML. */
export interface ContextGraphArtifact {
  name: string;
  path: string;
  definition: Record<string, unknown>;
}

/** `GET /context-graph/sources`: the served revision's artifacts. */
export interface ContextGraphSources {
  views: ContextGraphArtifact[];
  topics: ContextGraphArtifact[];
  automations: ContextGraphArtifact[];
  /** Analytics agents (`.agentic.yml`), the agent kind the compile records. */
  agents: ContextGraphArtifact[];
  /** Verified query paths; the graph draws one only where it is referenced. */
  sql_queries: string[];
}
