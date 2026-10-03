/**
 * Pure fold from the airway SSE event stream into the run-page view
 * model (phase bar + per-resource grid).
 *
 * All airway run-page logic lives here; the components are presentation
 * only. Mirrors the spirit of `analyticsSteps.ts` — deterministic,
 * unit-tested, no React.
 */

import type { AirwayEvent, ResourceContract } from "@/services/api/airway";

export type PhaseState = "pending" | "active" | "done";

export type ResourceStatus =
  | "pending"
  | "extracting"
  | "normalizing"
  | "loading"
  | "done"
  | "error";

export type ResourceRow = {
  table: string;
  /** Set for child tables produced by relational normalization. */
  parent?: string;
  /**
   * The source contract this resource was pulled under, from
   * `pipeline_plan`.
   *
   * Three states, not two — the distinction matters and the UI must keep
   * it:
   * - `undefined` — the stream said nothing. Either an old run recorded
   *   before contracts rode on `pipeline_plan`, or a normalized child
   *   table, which is not a source resource and has no contract of its
   *   own. Render nothing.
   * - `{ mutability: "undeclared" }` — the connector was asked and
   *   declared nothing. Render it as such.
   * - anything else — a declared vendor fact.
   *
   * Collapsing the first two into "undeclared" would claim a connector
   * said something it was never asked; collapsing `undeclared` into
   * `opaque` would turn a gap into a guarantee.
   */
  contract?: ResourceContract;
  rowsExtracted?: number;
  rowsNormalized?: number;
  rowsLoaded?: number;
  status: ResourceStatus;
  /** Failure reason when `status === "error"` (from `resource_failed`). */
  error?: string;
  /** Per-phase emit timestamps (ISO, from the event `ts`) — drive the
   *  run-timeline Gantt. Set once (idempotent). */
  extractStartedAt?: string;
  extractEndedAt?: string;
  normalizeStartedAt?: string;
  normalizeEndedAt?: string;
  loadStartedAt?: string;
  loadEndedAt?: string;
};

export type AirwayRunStatus = "running" | "done" | "completed_with_errors" | "failed" | "cancelled";

export type AirwayRunView = {
  pipelineName?: string;
  loadId?: string;
  /** Destination name from `pipeline_plan` (lineage skeleton). */
  destination?: string;
  phase: { extract: PhaseState; normalize: PhaseState; load: PhaseState };
  /** Parents first; each parent's children follow immediately after it. */
  resources: ResourceRow[];
  status: AirwayRunStatus;
  error?: string;
  /** Resources skipped via `resource_failed` (run still completed). */
  failedResources: { table: string; error: string }[];
  /** `SchemaEvolved` payload, surfaced as a diff badge. */
  schemaChanges?: unknown;
  durationMs?: number;
  /** Run span (ISO) for the timeline axis: `load_started` →
   *  `load_completed`/`pipeline_error`/`cancelled`. */
  startedAt?: string;
  endedAt?: string;
};

function emptyView(): AirwayRunView {
  return {
    phase: { extract: "pending", normalize: "pending", load: "pending" },
    resources: [],
    status: "running",
    failedResources: []
  };
}

/**
 * Per-`view` table → row index. The reducer re-folds the whole event
 * buffer on every SSE tick; without this, each event's
 * `upsertResource` did an O(R) `view.resources.find`, making a fold
 * O(N·R) and the streamed run O(N²) (felt on a 200-table Toast pull,
 * now also replayed by the Pipeline Overview tab). Keyed by the fresh
 * `view` a single `reduceAirwayEvents` call owns, so the function
 * stays pure and the WeakMap entry is GC'd with the view. Table names
 * are unique across the tree (the normalizer's path naming), so one
 * entry per `table` is unambiguous.
 */
const rowIndex = new WeakMap<AirwayRunView, Map<string, ResourceRow>>();

function indexFor(view: AirwayRunView): Map<string, ResourceRow> {
  let m = rowIndex.get(view);
  if (!m) {
    m = new Map();
    rowIndex.set(view, m);
  }
  return m;
}

/**
 * Find-or-create the row for `table`, nesting it correctly.
 *
 * The relational normalizer names child tables `parent__child`
 * (recursively, e.g. `orders__checks__selections`), so the hierarchy
 * is fully derivable from the name. We nest by name rather than by
 * waiting for `normalize_completed.child_tables`, because in the
 * streaming path that event arrives *after* a child's
 * `table_load_started`/`load_progress` events — relying on it left
 * children stranded as top-level roots. Name-based nesting is
 * order-independent, so every event places a child under its parent
 * regardless of arrival order.
 */
function upsertResource(view: AirwayRunView, table: string): ResourceRow {
  const sep = table.lastIndexOf("__");
  if (sep > 0) {
    const parent = table.slice(0, sep);
    // Ensure the (possibly deep) ancestor chain exists first.
    upsertResource(view, parent);
    return upsertChild(view, parent, table);
  }
  const m = indexFor(view);
  let row = m.get(table);
  if (!row) {
    row = { table, status: "pending" };
    view.resources.push(row);
    m.set(table, row);
  }
  return row;
}

/**
 * Insert a child row directly after its parent (or after the parent's
 * existing trailing children), preserving the "parents first, children
 * nested under parent" ordering the grid renders.
 */
function upsertChild(view: AirwayRunView, parent: string, table: string): ResourceRow {
  const m = indexFor(view);
  const existing = m.get(table);
  if (existing) return existing;

  const row: ResourceRow = { table, parent, status: "normalizing" };
  // Find the parent at ANY depth — a grandchild's parent is itself a
  // child (has `.parent`), so the old `&& !r.parent` guard missed it
  // and the row got appended to the end of the array, landing visually
  // under whatever unrelated root preceded it.
  const parentIdx = view.resources.findIndex((r) => r.table === parent);
  if (parentIdx === -1) {
    view.resources.push(row);
    m.set(table, row);
    return row;
  }
  // Insert after the parent AND its entire existing subtree so the
  // array stays pre-order (parent, then all descendants, contiguously).
  // Every descendant's table name starts with `${parent}__` — the
  // normalizer's deterministic path naming — so the prefix test spans
  // the whole subtree, not just direct children.
  const subtreePrefix = `${parent}__`;
  let insertAt = parentIdx + 1;
  while (
    insertAt < view.resources.length &&
    view.resources[insertAt].table.startsWith(subtreePrefix)
  ) {
    insertAt += 1;
  }
  view.resources.splice(insertAt, 0, row);
  m.set(table, row);
  return row;
}

function markResourceFailed(view: AirwayRunView, table: string, error: string): void {
  const row = upsertResource(view, table);
  row.status = "error";
  row.error = error;
  if (!view.failedResources.some((f) => f.table === table)) {
    view.failedResources.push({ table, error });
  }
}

function markRunFailed(view: AirwayRunView, error: string, ts: string | undefined): void {
  view.status = "failed";
  view.error = error;
  view.endedAt ??= ts;
  for (const r of view.resources) {
    if (r.status !== "done") r.status = "error";
  }
}

/**
 * Fold the (chronologically ordered) event list into the view model.
 *
 * Pure and idempotent over a prefix: feeding the first N events always
 * yields the same view, so the hook can re-reduce the accumulated
 * buffer on every SSE tick without tracking deltas.
 */
export function reduceAirwayEvents(events: AirwayEvent[]): AirwayRunView {
  const view = emptyView();

  for (const ev of events) {
    switch (ev.type) {
      case "load_started": {
        view.pipelineName = ev.payload.pipeline_name;
        view.loadId = ev.payload.load_id;
        view.phase.extract = "active";
        view.status = "running";
        view.startedAt ??= ev.payload.ts;
        break;
      }
      case "pipeline_plan": {
        view.pipelineName = ev.payload.pipeline_name;
        view.loadId = ev.payload.load_id;
        view.destination = ev.payload.destination;
        // Render the full skeleton immediately: every resource as
        // `pending` so the grid/lineage view isn't empty while the
        // first (slow) extract runs.
        //
        // `contracts` (when the stream carries it) has one entry per
        // planned resource, keyed by name rather than by position, so a
        // future divergence in ordering can't silently mis-attribute a
        // contract to the wrong resource. A resource with no entry keeps
        // `contract === undefined` — see `ResourceRow.contract`.
        const contracts = new Map<string, ResourceContract>(
          (ev.payload.contracts ?? []).map((c) => [c.resource, c])
        );
        for (const table of ev.payload.resources) {
          const row = upsertResource(view, table);
          const contract = contracts.get(table);
          if (contract) row.contract = contract;
        }
        break;
      }
      case "extract_started": {
        view.pipelineName = ev.payload.pipeline_name;
        view.phase.extract = "active";
        const row = upsertResource(view, ev.payload.table);
        row.extractStartedAt ??= ev.payload.ts;
        // Show the resource as in-flight immediately. Don't downgrade a
        // row that already advanced (a fast resource may complete
        // before every up-front extract_started is reduced).
        if (row.status === "pending") row.status = "extracting";
        break;
      }
      case "extract_progress": {
        view.phase.extract = "active";
        const row = upsertResource(view, ev.payload.table);
        row.rowsExtracted = ev.payload.rows_so_far;
        if (row.status === "pending" || row.status === "extracting") {
          row.status = "extracting";
        }
        break;
      }
      case "extract_completed": {
        view.pipelineName = ev.payload.pipeline_name;
        const row = upsertResource(view, ev.payload.table);
        row.rowsExtracted = ev.payload.rows_extracted;
        row.extractEndedAt ??= ev.payload.ts;
        // Extracted but not yet normalized — still in flight overall.
        if (row.status === "pending" || row.status === "extracting") {
          row.status = "extracting";
        }
        break;
      }
      case "normalize_started": {
        view.phase.extract = "done";
        view.phase.normalize = "active";
        const row = upsertResource(view, ev.payload.table);
        row.normalizeStartedAt ??= ev.payload.ts;
        if (row.status !== "error" && row.status !== "done") {
          row.status = "normalizing";
        }
        break;
      }
      case "normalize_completed": {
        const row = upsertResource(view, ev.payload.table);
        row.rowsNormalized = ev.payload.rows_normalized;
        row.normalizeEndedAt ??= ev.payload.ts;
        row.status = "normalizing";
        view.phase.extract = "done";
        view.phase.normalize = "active";
        // `child_tables` lists *all* descendants (e.g. both
        // `orders__checks` and `orders__checks__selections`). Route
        // each through the name-aware `upsertResource` so deep
        // descendants nest under their *immediate* parent — attaching
        // them all directly under the resource would both flatten the
        // tree and duplicate rows already nested by name.
        for (const child of ev.payload.child_tables) {
          upsertResource(view, child);
        }
        break;
      }
      case "destination_load_started": {
        view.phase.extract = "done";
        view.phase.normalize = "done";
        view.phase.load = "active";
        for (const t of ev.payload.tables) {
          const row = indexFor(view).get(t) ?? upsertResource(view, t);
          row.status = "loading";
        }
        break;
      }
      case "table_load_started": {
        view.phase.extract = "done";
        view.phase.normalize = "done";
        view.phase.load = "active";
        const row = upsertResource(view, ev.payload.table);
        row.loadStartedAt ??= ev.payload.ts;
        if (row.status !== "error" && row.status !== "done") {
          row.status = "loading";
        }
        break;
      }
      case "load_progress": {
        view.phase.load = "active";
        const row = upsertResource(view, ev.payload.table);
        row.rowsLoaded = ev.payload.rows_written;
        if (row.status !== "error" && row.status !== "done") {
          row.status = "loading";
        }
        break;
      }
      case "table_load_failed": {
        markResourceFailed(view, ev.payload.table, ev.payload.error);
        break;
      }
      case "table_loaded": {
        const row = upsertResource(view, ev.payload.table);
        row.rowsLoaded = ev.payload.rows;
        row.loadEndedAt ??= ev.payload.ts;
        if (row.status !== "error") row.status = "done";
        break;
      }
      case "load_completed": {
        for (const [table, rows] of Object.entries(ev.payload.rows_loaded)) {
          const row = indexFor(view).get(table) ?? upsertResource(view, table);
          row.rowsLoaded = rows;
        }
        // Everything that didn't fail is terminal-success now; skipped
        // resources stay `error`.
        for (const r of view.resources) {
          if (r.status !== "error") r.status = "done";
        }
        view.phase.extract = "done";
        view.phase.normalize = "done";
        view.phase.load = "done";
        view.status = view.failedResources.length > 0 ? "completed_with_errors" : "done";
        view.durationMs = ev.payload.duration_ms;
        view.endedAt ??= ev.payload.ts;
        break;
      }
      case "resource_failed": {
        markResourceFailed(view, ev.payload.table, ev.payload.error);
        break;
      }
      case "schema_evolved": {
        view.schemaChanges = ev.payload.changes;
        break;
      }
      case "pipeline_error": {
        markRunFailed(view, ev.payload.error, ev.payload.ts);
        break;
      }
      case "cancelled": {
        view.status = "cancelled";
        view.endedAt ??= ev.payload.ts;
        break;
      }
      // Coordinator failed the task before the engine ran (secrets /
      // connector / destination / config resolution in execute_airway).
      // No engine `pipeline_error` is emitted on this path, so without
      // this the run page stays blank on a pre-processing failure.
      case "task_failed": {
        markRunFailed(view, ev.payload.error, ev.payload.ts);
        break;
      }
    }
  }

  return view;
}
