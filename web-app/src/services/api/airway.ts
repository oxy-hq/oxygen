/**
 * Client for the `/agentic-airway` HTTP surface (mounted per workspace).
 *
 * Airway is queue-driven like automation: `startRun` seeds the run and
 * the runtime coordinator drives it; events stream over the shared
 * domain-agnostic SSE endpoint, routed by `source_type = "airway"`.
 * Mirrors `automations.ts` — same SSE shape (`event:` field for
 * the type, `data:` for the payload).
 */

import { fetchEventSource } from "@microsoft/fetch-event-source";

import { apiBaseURL } from "../env";
import { apiClient } from "./axios";

// ── Wire types ─────────────────────────────────────────────────────────────

export interface SpApiMarketplace {
  id: string;
  country: string;
  label: string;
}

export type StartAirwayRequest = {
  /** Path to a `.airway.yml`, relative to the workspace root. */
  pipeline_ref: string;
  /** Optional variables rendered into the pipeline YAML at run time. */
  variables?: Record<string, unknown>;
  /** Optional conversation-thread association. */
  thread_id?: string;
  /** Explicit subset of resources (tables) to run, overriding the spec's
   *  `resources` — used by "retry failed tables". Omit to run the whole spec. */
  resources?: string[];
};

export type BackfillAirwayRequest = {
  /** Path to a `.airway.yml`, relative to the workspace root. */
  pipeline_ref: string;
  /** Inclusive lower bound (ISO 8601 / RFC3339). Window is half-open `[from, to)`. */
  from: string;
  /** Exclusive upper bound (ISO 8601 / RFC3339). */
  to: string;
  /** Optional subset of resources to backfill. Omit = whole spec; the
   *  non-date-windowed resources just ignore the window. */
  resources?: string[];
};

export type ChunkGranularity = "month" | "week" | "day";

export type ChunkedBackfillRequest = {
  /** Path to a `.airway.yml`, relative to the workspace root. */
  pipeline_ref: string;
  /** Inclusive lower bound (ISO 8601 / RFC3339). Window is half-open `[from, to)`. */
  from: string;
  /** Exclusive upper bound (ISO 8601 / RFC3339). */
  to: string;
  /** Chunk size — one checkpoint per chunk. */
  granularity: ChunkGranularity;
  /** Max chunks to run at once (default 4 server-side, clamped 1..=16). */
  concurrency?: number;
};

export type ChunkedBackfillResponse = {
  /** The backfill range created for this window. Poll coverage(range_id), or
   *  list ranges, for progress. */
  range_id: string;
  /** How many chunks the window split into (also the checkpoint count). */
  chunk_count: number;
};

export type ResumeBackfillRequest = {
  /** The backfill range to resume — re-runs its not-`done` chunks at the
   *  range's stored concurrency. */
  range_id: string;
};

// ── Cursor reset (rewind, not drop) ────────────────────────────────────────

export type ResetCursorsRequest = {
  /** Path to a `.airway.yml`, relative to the workspace root. */
  pipeline_ref: string;
  /**
   * Resources to rewind. Omitted or empty means **every** resource holding a
   * cursor — the server reads an empty list the wider way, so `[]` is never
   * "none".
   *
   * The UI never sends `[]`. It resolves on the server, against the cursors
   * held *then* — which can include one that appeared after the picker
   * loaded, so "every resource" would clear a resource nobody was shown. A
   * list naming every held cursor is judged exactly as `[]` is (no
   * `UnknownOwnership` noise, nothing having been narrowed) and clears only
   * the names it carries.
   */
  resources?: string[];
  /** Proceed past a convergence refusal. The server logs the overridden
   *  reasons. */
  force?: boolean;
};

/** `200` — the cursors moved. */
export type ResetCursorsCleared = {
  kind: "cleared";
  /** Resources whose cursor was removed, sorted. */
  cleared: string[];
  /** Resources named by the caller that held no cursor — never run, or a typo.
   *  Not an error, but the two are indistinguishable without it. */
  not_held: string[];
};

/**
 * `409` — the server understood and declined, because re-pulling these
 * resources would duplicate rows rather than converge.
 *
 * `reasons` is the backend's own prose, already rendered, one entry per table
 * or scoping problem. It names the table, says why, and for an unscopeable
 * reset gives **both readings**. Render it verbatim: an operator deciding
 * whether to `force` is deciding on exactly these sentences, and a summary of
 * them is a decision made on less than the facts.
 */
export type ResetCursorsRefused = {
  kind: "refused";
  reasons: string[];
};

/**
 * A refusal is not a failure — the route says so itself — so it comes back as a
 * value rather than a thrown error. Left as an exception it would reach the UI
 * as an `AxiosError` whose `.message` is `"Request failed with status code
 * 409"`, and the careful reasons would never be seen.
 */
export type ResetCursorsOutcome =
  | ResetCursorsCleared
  | ResetCursorsRefused
  | ResetCursorsPipelineRunning;

/**
 * `409` with `error: "pipeline_running"` — something holds the pipeline's
 * single-flight lease (usually a run, possibly another cursor reset), so the
 * server did not reset anything. `message` says which; branch on nothing else.
 *
 * Distinct from {@link ResetCursorsRefused} because it **cannot be forced**:
 * `force` overrides the convergence judgement, and this is not a judgement —
 * it is an observed holder, and for a run, one whose cursor save the reset
 * would break. Offering the override here would offer a button that cannot
 * work. Wait for the holder (or cancel it, when it is a run), then rewind.
 */
export type ResetCursorsPipelineRunning = {
  kind: "pipeline_running";
  /** The lease holder: a run id, or `cursor-reset:<uuid>` for another reset. */
  run_id: string;
  /** The server's explanation, rendered verbatim. */
  message: string;
};

/** One chunk's coverage row (mirrors a `backfill_checkpoints` row). */
export type CoverageChunk = {
  /** ISO 8601. Half-open `[period_start, period_end)`. */
  period_start: string;
  period_end: string;
  /** `pending` | `running` | `done` | `completed_with_errors` | `failed` | `cancelled` | `timed_out`. */
  status: string;
  run_id: string | null;
  row_count: number | null;
  attempts: number;
  error: string | null;
};

export type CoverageSummary = {
  total: number;
  done: number;
  /** Loaded envelope: min/max over `done` chunks — NOT necessarily gap-free;
   *  null when nothing is done. Check `missing` / per-chunk status for gaps. */
  loaded_from: string | null;
  loaded_to: string | null;
  missing: number;
};

export type CoverageReport = {
  pipeline_ref: string;
  /** The range this report covers, or null for a pipeline-wide aggregate. */
  range_id: string | null;
  chunks: CoverageChunk[];
  summary: CoverageSummary;
};

/** One backfill range plus its chunk tally (mirrors `BackfillRangeInfo`). The
 *  parent of a set of coverage chunks; the ranges list backs the gantt. */
export type BackfillRangeInfo = {
  id: string;
  pipeline_ref: string;
  /** Requested half-open window `[requested_from, requested_to)` (ISO 8601). */
  requested_from: string;
  requested_to: string;
  granularity: string;
  concurrency: number;
  created_by: string | null;
  /** Rollup: `running` | `done` | `degraded` | `failed` | `cancelled`. */
  status: string;
  created_at: string;
  updated_at: string;
  total: number;
  done: number;
  missing: number;
};

export type AirwayRunSummary = {
  run_id: string;
  status: string;
  /** ISO 8601 (`chrono::DateTime`). */
  created_at: string;
  updated_at: string;
  /** Backfill window `[from, to)` (ISO 8601) if this run was a backfill; null
   *  for a normal/incremental run. Lets the list show which period it covers. */
  backfill_from: string | null;
  backfill_to: string | null;
};

/** `.airway.yml` file ref (parity with `AutomationFile`). */
/**
 * SHA-256 of a file's bytes, hex, truncated to 32 chars.
 *
 * Truncated because it becomes a path segment and the server caps the id at
 * 128 characters; 128 bits of a SHA-256 is far past any collision concern for
 * a workspace's monthly reports. Hex so it satisfies the server's
 * `[A-Za-z0-9_-]` rule without encoding.
 */
async function contentHash(file: File): Promise<string> {
  // `crypto.subtle` is secure-context only: on a self-hosted `oxy serve`
  // reached over plain HTTP it is `undefined`, and the bare call died with an
  // opaque `TypeError` the moment a file was dropped. Named here instead,
  // because the cause is the page's origin and no amount of retrying helps.
  if (!globalThis.crypto?.subtle) {
    throw new Error(
      "Uploading needs a secure context (HTTPS or localhost) to hash the file. " +
        "This page is served over plain HTTP, so the browser withholds crypto.subtle."
    );
  }
  const digest = await crypto.subtle.digest("SHA-256", await file.arrayBuffer());
  return Array.from(new Uint8Array(digest))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("")
    .slice(0, 32);
}

export type AirwayFile = {
  path: string;
  path_b64: string;
  /**
   * `source.kind` — what source-specific surfaces key on (the report-upload
   * tab is only meaningful for `ubereats`).
   *
   * Comes from the compiled definition when there is one, and from a direct
   * YAML read on the filesystem fallback, so an un-promoted workspace answers
   * the same kind as a promoted one. This used to say the fallback omitted
   * it — true before the fallback learned to read it, and the reason the tab
   * was missing in local mode.
   *
   * Absent now only when the file could not be read or did not parse; a
   * surface gated on this stays hidden in that case rather than appearing and
   * then failing.
   */
  source_kind?: string;
};

/** One report the server accepted and wrote to the landing zone. */
export type UploadedReport = {
  /** `s3://…` — the same spelling the pipeline's `base_path` uses. */
  location: string;
  report_year: number;
  report_month: number;
  /** Rows the report yielded under validation. Zero is a valid empty report. */
  rows: number;
};

/** A column surfaced by source table discovery. */
export type DiscoveredColumn = {
  name: string;
  /** Native source type (e.g. ClickHouse `Int64`, `Nullable(String)`). */
  data_type: string;
};

/** A table surfaced by source table discovery, columns in declaration order. */
export type DiscoveredTable = {
  name: string;
  columns: DiscoveredColumn[];
};

/** Live credentials for source introspection (not persisted). */
export type DiscoverSourceRequest = {
  /** Source kind — only introspectable kinds (`clickhouse`) are wired. */
  kind: string;
  /** Connector-specific connection fields. */
  config: Record<string, unknown>;
};

/**
 * How a source resource's rows behave, as declared by its connector.
 *
 * `undeclared` is deliberately **not** `opaque`: `opaque` is a checked
 * vendor fact ("this source exposes no version"), `undeclared` is a gap
 * nobody has filled. Airway's own default for an undeclared resource is
 * `opaque`, and collapsing the two here would make a gap read as a
 * guarantee. Mirrors `agentic_airway::contract::ContractMutability`.
 */
export type ContractMutability = "immutable" | "versioned" | "opaque" | "undeclared";

/**
 * Frontend mirror of `agentic_airway::contract::ResourceContract` — one
 * source resource's airway `SourceContract`, flattened for the wire.
 *
 * Every field but `resource`/`mutability` is `null` when
 * `mutability === "undeclared"`; nothing may be asserted on an undeclared
 * resource's behalf. For a declared contract, `null` is a real fact (e.g.
 * `restatement_window_ms: null` = declares no restatement window), so
 * `mutability` is what tells the two apart.
 *
 * Durations are milliseconds — seconds would truncate a sub-second
 * `cursor_lag` to `0`, which reads as "declares no lag".
 */
export type ResourceContract = {
  resource: string;
  mutability: ContractMutability;
  /** Source-side version field (the vendor's API path). */
  version_field: string | null;
  /** Landed column the destination's version guard compares on. */
  version_column: string | null;
  cursor_tracks_modification: boolean | null;
  restatement_window_ms: number | null;
  cursor_lag_ms: number | null;
  /** `cursor_lag + restatement_window` — what a pull rewinds by. */
  rewind_ms: number | null;
  requires_partition_repull: boolean | null;
};

/**
 * Frontend mirror of `agentic_airway::AirwayEvent`. The backend tags
 * with `event_type` (snake_case) and the runtime splits that into the
 * SSE `event:` field; `data:` carries the full payload (the tag is
 * echoed inside it too, harmlessly).
 */
export type AirwayEvent =
  | {
      type: "load_started";
      payload: { pipeline_name: string; load_id: string };
    }
  | {
      type: "pipeline_plan";
      payload: {
        pipeline_name: string;
        load_id: string;
        resources: string[];
        destination: string;
        /**
         * One entry per planned resource, in plan order — undeclared ones
         * included and labelled.
         *
         * Optional because runs recorded before the field existed replay
         * verbatim. Absent or empty means "this stream carried no contract
         * information", which is NOT the same as "everything is undeclared":
         * the grid renders nothing for those rows rather than asserting a
         * fact about a run it cannot see.
         */
        contracts?: ResourceContract[];
      };
    }
  | {
      type: "extract_started";
      payload: { pipeline_name: string; load_id: string; table: string };
    }
  | {
      type: "extract_progress";
      payload: {
        pipeline_name: string;
        load_id: string;
        table: string;
        rows_so_far: number;
      };
    }
  | {
      type: "extract_completed";
      payload: {
        pipeline_name: string;
        load_id: string;
        table: string;
        rows_extracted: number;
      };
    }
  | {
      type: "normalize_started";
      payload: { pipeline_name: string; load_id: string; table: string };
    }
  | {
      type: "normalize_completed";
      payload: {
        pipeline_name: string;
        load_id: string;
        table: string;
        rows_normalized: number;
        child_tables: string[];
      };
    }
  | {
      type: "destination_load_started";
      payload: { pipeline_name: string; load_id: string; tables: string[] };
    }
  | {
      type: "table_load_started";
      payload: { pipeline_name: string; load_id: string; table: string };
    }
  | {
      type: "load_progress";
      payload: {
        pipeline_name: string;
        load_id: string;
        table: string;
        rows_written: number;
      };
    }
  | {
      type: "table_load_failed";
      payload: {
        pipeline_name: string;
        load_id: string;
        table: string;
        error: string;
      };
    }
  | {
      type: "table_loaded";
      payload: {
        pipeline_name: string;
        load_id: string;
        table: string;
        rows: number;
      };
    }
  | {
      type: "load_completed";
      payload: {
        pipeline_name: string;
        load_id: string;
        tables: string[];
        /** Per-table row counts written to the destination. */
        rows_loaded: Record<string, number>;
        duration_ms: number;
      };
    }
  | {
      type: "schema_evolved";
      payload: { pipeline_name: string; changes: unknown };
    }
  | {
      type: "resource_failed";
      payload: {
        pipeline_name: string;
        load_id: string;
        table: string;
        error: string;
      };
    }
  | {
      type: "pipeline_error";
      payload: {
        pipeline_name: string;
        load_id: string | null;
        error: string;
      };
    }
  | {
      type: "cancelled";
      payload: { pipeline_name: string; load_id: string };
    }
  // Coordinator-level failure (not an engine event): emitted when
  // `execute_airway` errors *before* the worker starts — secrets,
  // connector/destination resolution, config/spec issues. Without
  // handling this the run page stays empty on a pre-processing error.
  | {
      type: "task_failed";
      payload: {
        task_id?: string;
        attempt?: number;
        spec_kind?: string | null;
        step_name?: string | null;
        error: string;
      };
    };

type AirwayEventType = AirwayEvent["type"];

const KNOWN_EVENTS = new Set<AirwayEventType>([
  "load_started",
  "pipeline_plan",
  "extract_started",
  "extract_progress",
  "extract_completed",
  "normalize_started",
  "normalize_completed",
  "destination_load_started",
  "table_load_started",
  "load_progress",
  "table_load_failed",
  "table_loaded",
  "load_completed",
  "schema_evolved",
  "resource_failed",
  "pipeline_error",
  "cancelled",
  "task_failed"
]);

// ── Service ────────────────────────────────────────────────────────────────

export class AirwayService {
  private static base(projectId: string): string {
    return `/${projectId}/agentic-airway`;
  }

  static async startRun(
    projectId: string,
    request: StartAirwayRequest
  ): Promise<{ run_id: string }> {
    const { data } = await apiClient.post(`${AirwayService.base(projectId)}/runs`, request);
    return data;
  }

  static async backfillRun(
    projectId: string,
    request: BackfillAirwayRequest
  ): Promise<{ run_id: string }> {
    const { data } = await apiClient.post(`${AirwayService.base(projectId)}/backfill`, request);
    return data;
  }

  /**
   * Start a resumable chunked backfill. Returns immediately with the chunk
   * count; the server drives the chunks detached. Poll {@link coverage} for
   * progress. Re-invoking the same window resumes (skips `done` chunks).
   */
  static async chunkedBackfill(
    projectId: string,
    request: ChunkedBackfillRequest
  ): Promise<ChunkedBackfillResponse> {
    const { data } = await apiClient.post(
      `${AirwayService.base(projectId)}/chunked-backfill`,
      request
    );
    return data;
  }

  /**
   * Resume a chunked backfill: re-run only the not-`done` chunks, read from
   * coverage (no window/granularity needed). Returns the count it will re-run;
   * `chunk_count` is the number of missing chunks. Poll {@link coverage}.
   */
  static async resumeBackfill(
    projectId: string,
    request: ResumeBackfillRequest
  ): Promise<ChunkedBackfillResponse> {
    const { data } = await apiClient.post(
      `${AirwayService.base(projectId)}/resume-backfill`,
      request
    );
    return data;
  }

  /** Coverage for a single backfill range (per-chunk status + rollup). */
  static async coverage(projectId: string, rangeId: string): Promise<CoverageReport> {
    const { data } = await apiClient.get(`${AirwayService.base(projectId)}/coverage`, {
      params: { range_id: rangeId }
    });
    return data;
  }

  /** List a pipeline's backfill ranges (newest first) with each range's chunk
   *  tally — the source for the ranges gantt. */
  static async listBackfillRanges(
    projectId: string,
    pipelineRef: string
  ): Promise<BackfillRangeInfo[]> {
    const { data } = await apiClient.get(`${AirwayService.base(projectId)}/backfill-ranges`, {
      params: { pipeline_ref: pipelineRef }
    });
    return data;
  }

  static async cancelRun(projectId: string, runId: string): Promise<void> {
    await apiClient.post(`${AirwayService.base(projectId)}/runs/${runId}/cancel`);
  }

  /**
   * Reset a pipeline's schema: drop its destination tables and clear the stored
   * schema + incremental cursor, so a later run re-infers a fresh schema. This is
   * destructive (the tables' data is dropped) — confirm first, then backfill to
   * repopulate. Returns the table names that were dropped.
   */
  static async resetSchema(
    projectId: string,
    pipelineRef: string
  ): Promise<{ dropped_tables: string[] }> {
    const { data } = await apiClient.post(`${AirwayService.base(projectId)}/reset-schema`, {
      pipeline_ref: pipelineRef
    });
    return data;
  }

  /**
   * Resources holding a cursor for this pipeline, sorted — the names
   * {@link resetCursors} accepts.
   *
   * Offered rather than asked for. The scope takes the raw cursor keys, and the
   * nearest thing the UI can otherwise reach is a table name from a run's
   * lineage, which is that name put through the normalizer. Where the two
   * diverge, a reset scoped to the table name clears nothing and says so only
   * afterwards, in `not_held`.
   */
  static async resourceCursors(projectId: string, pipelineRef: string): Promise<string[]> {
    const { data } = await apiClient.get<{ resources: string[] }>(
      `${AirwayService.base(projectId)}/resource-cursors`,
      { params: { pipeline_ref: pipelineRef } }
    );
    return data.resources ?? [];
  }

  /**
   * Rewind a pipeline's incremental cursors, **keeping every landed row**. The
   * non-destructive sibling of {@link resetSchema}: the next run re-pulls the
   * named resources from their `default_start`; nothing is dropped.
   *
   * Returns a refusal as a value rather than throwing it — see
   * {@link ResetCursorsOutcome}. Everything else (400 / 500 / 503) still
   * throws.
   */
  static async resetCursors(
    projectId: string,
    request: ResetCursorsRequest
  ): Promise<ResetCursorsOutcome> {
    const res = await apiClient.post(`${AirwayService.base(projectId)}/reset-cursors`, request, {
      // 409 is the server declining a well-formed request, not an error. Left
      // to axios' default it rejects, and the reasons the operator has to read
      // are replaced by "Request failed with status code 409".
      validateStatus: (s) => (s >= 200 && s < 300) || s === 409
    });

    if (res.status === 409) {
      const body = res.data as
        | { error?: unknown; run_id?: unknown; message?: unknown; reasons?: unknown }
        | undefined;
      if (body?.error === "pipeline_running") {
        return {
          kind: "pipeline_running",
          run_id: typeof body.run_id === "string" ? body.run_id : "",
          message:
            typeof body.message === "string"
              ? body.message
              : // Only reached on version skew, where the holder kind is
                // least knowable — so claim none.
                "Something holds this pipeline's lease (a run, or another reset). Wait for it to finish — or cancel it, if it is a run — then rewind."
        };
      }
      const reasons: unknown = body?.reasons;
      const parsed =
        Array.isArray(reasons) && reasons.every((r) => typeof r === "string")
          ? (reasons as string[])
          : [];
      // A refusal with nothing in it would render as an empty panel — the exact
      // silent swallow this whole path exists to prevent. Better to fail loudly
      // with the body than to show a refusal that explains nothing.
      if (parsed.length === 0) {
        throw new Error(
          `Cursor reset was refused, but the server sent no reasons: ${JSON.stringify(res.data)}`
        );
      }
      return { kind: "refused", reasons: parsed };
    }

    const body = res.data as { cleared?: string[]; not_held?: string[] } | undefined;
    return {
      kind: "cleared",
      cleared: body?.cleared ?? [],
      not_held: body?.not_held ?? []
    };
  }

  static async listRuns(
    projectId: string,
    pipelineRef: string,
    limit = 50
  ): Promise<AirwayRunSummary[]> {
    const { data } = await apiClient.get(`${AirwayService.base(projectId)}/runs`, {
      params: { pipeline_ref: pipelineRef, limit }
    });
    return data;
  }

  /**
   * Upload one payment-details report into a pipeline's landing zone.
   *
   * `workflowId` is the **content hash**, not a random id: the object key is
   * the loader's merge identity, so hashing the bytes makes re-uploading the
   * same file land on the same key and merge, while a different file gets its
   * own. A random id would turn every re-drop into a duplicate — the same
   * replace-by-file-sha reasoning the bookkeeping app's importer uses.
   *
   * `period` is optional. Given, it wins; omitted, the server reads it from
   * the file name (`2026.08 UberEats SF.csv`). Explicit is better when the
   * caller knows — a file name is the part of a report that can lie — but
   * requiring it would make a correctly-named file need a form field for
   * something already in the name.
   */
  static async uploadReport(
    projectId: string,
    pipelineRef: string,
    file: File,
    period: { year: number; month: number } | undefined,
    onProgress?: (loaded: number, total: number) => void
  ): Promise<UploadedReport> {
    const form = new FormData();
    form.append("file", file, file.name);
    form.append("pipeline_ref", pipelineRef);
    form.append("workflow_id", await contentHash(file));
    // Both or neither — the server refuses half a period, because taking one
    // half and guessing the other stamps a month nobody named. Omitted
    // entirely, it derives the period from the file name (`2026.08 …`).
    if (period) {
      form.append("report_year", String(period.year));
      form.append("report_month", String(period.month));
    }

    const { data } = await apiClient.post<UploadedReport>(
      `/${projectId}/source-uploads/reports`,
      form,
      {
        onUploadProgress: (e) => {
          if (onProgress && e.total) onProgress(e.loaded, e.total);
        }
      }
    );
    return data;
  }

  /**
   * Marketplaces the `sp_api` connector can reach, for the wizard's picker.
   *
   * Served rather than hardcoded here: `SpApiSource` pins the North America
   * endpoint today, and when it learns to take a host the Rust list widens. A
   * copy in this file would then be the only thing refusing a marketplace the
   * connector accepts, and nothing would fail to point at it.
   */
  static async listSpApiMarketplaces(projectId: string): Promise<SpApiMarketplace[]> {
    const { data } = await apiClient.get(`/${projectId}/airway-pipelines/sp-api/marketplaces`);
    return data;
  }

  static async listFiles(projectId: string): Promise<AirwayFile[]> {
    // Served from the compile boundary at `/airway-pipelines` (FleetOk) so the
    // list renders on a stateless serve replica; `/agentic-airway` is IdeOnly.
    const { data } = await apiClient.get(`/${projectId}/airway-pipelines`);
    return data;
  }

  /**
   * Connect to a source with the supplied live credentials and list its
   * tables (with columns), for the New Pipeline table picker. Nothing is
   * persisted server-side.
   */
  static async discoverSourceTables(
    projectId: string,
    request: DiscoverSourceRequest
  ): Promise<DiscoveredTable[]> {
    const { data } = await apiClient.post(
      `${AirwayService.base(projectId)}/sources/discover`,
      request
    );
    return data.tables;
  }

  /**
   * Stream events for a run. `onEvent` fires per known event type;
   * resolves when the stream closes (terminal) or `signal` aborts.
   */
  static async streamEvents(
    projectId: string,
    runId: string,
    options: {
      onEvent: (event: AirwayEvent) => void;
      onOpen?: () => void;
      onClose?: () => void;
      onError?: (error: Error) => void;
      signal?: AbortSignal;
    }
  ): Promise<void> {
    const url = `${apiBaseURL}${AirwayService.base(projectId)}/runs/${runId}/events`;
    const token = localStorage.getItem("auth_token");
    await fetchEventSource(url, {
      method: "GET",
      headers: { Authorization: token ?? "" },
      openWhenHidden: true,
      signal: options.signal,
      async onopen(res) {
        if (res.status !== 200) {
          throw new Error(`SSE connection failed with status: ${res.status}`);
        }
        options.onOpen?.();
      },
      onmessage(ev) {
        if (!ev.event || !KNOWN_EVENTS.has(ev.event as AirwayEventType)) return;
        try {
          const payload = JSON.parse(ev.data);
          options.onEvent({
            type: ev.event as AirwayEventType,
            payload
          } as AirwayEvent);
        } catch (error) {
          console.error("Error parsing airway SSE data:", error);
        }
      },
      onerror(err) {
        console.error("airway SSE error:", err);
        options.onError?.(err);
        throw err;
      },
      onclose() {
        options.onClose?.();
      }
    });
  }
}
