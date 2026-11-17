/**
 * Each function's latest recorded call traces in one environment, folded to what
 * each run touched. See `GET /customer-apps/{id}/recent-traces` (the backend's
 * `admin/apps/recent_traces.rs`): the right-hand side of the console's app map.
 */
export interface RecentTraces {
  environment: string;
  /** The bound each function's `traces` was read under. */
  per_function: number;
  /** How long a trace is kept: nothing older can be here. */
  retention_days: number;
  /** Every function of the environment's build, in name order. A function with
   *  no recorded run in the window has an empty `traces`. */
  functions: FunctionTraces[];
}

export interface FunctionTraces {
  function: string;
  /** Newest first. */
  traces: TraceDigest[];
}

/** One recorded run, as the distinct shapes of what it called. */
export interface TraceDigest {
  invocation_id: string;
  created_at: string;
  /** `route` | `schedule` | `airway` | `webhook` | `manual`. */
  mode: string;
  /** The pager's rule, as `FunctionInvocation.failed`. */
  failed: boolean;
  /** Every call the run made, kept in its trace or not. */
  total_calls: number;
  /** Calls past the trace's cap, which `touches` cannot include. */
  dropped: number;
  touches: TraceTouch[];
}

/** Calls of one run that share a shape. Facets as `InvocationCall`'s. */
export interface TraceTouch {
  /** `query` | `query_stream` | `fetch` | `semantic` | `airway` | `warehouse` |
   *  `secrets` | `email` | `notify` | `storage` | `org` | `platform` | `tx` |
   *  `oltp` | `airhouse`. */
  family: string;
  op: string;
  /** SQL verb for a statement, HTTP method for a fetch. */
  verb?: string;
  /** Table, `namespace.table`, `database.table`, a fetch's host, a pipeline or
   *  a secret's name. */
  target?: string;
  status: "ok" | "error" | "held" | "refused";
  error_type?: string;
  /** How many of the run's kept calls had this shape. */
  calls: number;
}
