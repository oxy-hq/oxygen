import { AxiosError } from "axios";
import { apiClient } from "./axios";

type FilterValue = string | number | boolean | null;

interface BaseFilter {
  field: string;
}

export type DateRangeValue = string | Date;

export type SemanticQueryFilter =
  | (BaseFilter & {
      op: "eq" | "neq" | "gt" | "gte" | "lt" | "lte";
      value: FilterValue;
    })
  | (BaseFilter & {
      op: "in" | "not_in";
      values: FilterValue[];
    })
  | (BaseFilter & {
      op: "in_date_range" | "not_in_date_range";
      relative?: string;
      from?: DateRangeValue;
      to?: DateRangeValue;
    });

export interface SemanticQueryOrder {
  field: string;
  direction: "asc" | "desc";
}

type TimeDimension = {
  dimension: string;
  granularity?:
    | "year"
    | "quarter"
    | "month"
    | "week"
    | "day"
    | "hour"
    | "minute"
    | "second"
    | string;
};

interface SemanticQueryParams {
  topic?: string;
  measures?: string[];
  dimensions?: string[];
  timeDimensions?: TimeDimension[];
  filters?: SemanticQueryFilter[];
  orders?: SemanticQueryOrder[];
  limit?: number;
  variables?: Record<string, unknown>;
}

export interface SemanticQueryRequest {
  query: SemanticQueryParams;
  session_filters?: Record<string, unknown>;
  connections?: Record<string, unknown>;
  result_format?: "json" | "parquet";
}

export interface SemanticQueryCompileResponse {
  sql: string;
}

interface Dimension {
  name: string;
  type: string;
  description?: string;
  expr: string;
}

interface Measure {
  name: string;
  type: string;
  description?: string;
  expr?: string;
  induced?: boolean;
  promoted_from?: string;
}

export interface ViewResponse {
  view_name: string;
  name: string;
  description?: string;
  datasource?: string;
  table?: string;
  dimensions: Dimension[];
  measures: Measure[];
}

interface TopicResponse {
  name: string;
  description?: string;
  views: string[];
  base_view?: string;
}

export interface TopicDetailsResponse {
  topic: TopicResponse;
  views: ViewResponse[];
}

export type ExecuteSemanticQueryResponse =
  | string[][] // JSON format - returns array directly
  | { file_name: string; is_preagg: boolean; execution_time_ms: number }; // Parquet format

export class SemanticService {
  static async executeSemanticQuery(
    projectId: string,
    request: SemanticQueryRequest,
    branchName?: string
  ): Promise<ExecuteSemanticQueryResponse> {
    const { query, ...rest } = request;
    try {
      const response = await apiClient.post(
        `/${projectId}/semantic`,
        {
          ...query,
          ...rest
        },
        { params: branchName ? { branch: branchName } : {} }
      );
      return response.data;
    } catch (error) {
      if (error instanceof AxiosError && error.response?.data?.message) {
        throw new Error(error.response.data.message);
      }
      throw error;
    }
  }

  static async compileSemanticQuery(
    projectId: string,
    request: SemanticQueryRequest,
    branchName?: string
  ): Promise<SemanticQueryCompileResponse> {
    const { query, ...rest } = request;
    try {
      const response = await apiClient.post(
        `/${projectId}/semantic/compile`,
        {
          ...query,
          ...rest
        },
        { params: branchName ? { branch: branchName } : {} }
      );
      return response.data;
    } catch (error) {
      if (error instanceof AxiosError && error.response?.data?.message) {
        throw new Error(error.response.data.message);
      }
      throw error;
    }
  }

  static async getTopicDetails(
    projectId: string,
    filePathB64: string,
    branchName?: string
  ): Promise<TopicDetailsResponse> {
    try {
      const params = branchName ? { branch: branchName } : {};
      const response = await apiClient.get(`/${projectId}/semantic/topic/${filePathB64}`, {
        params
      });
      return response.data;
    } catch (error) {
      if (error instanceof AxiosError && error.response?.data?.message) {
        throw new Error(error.response.data.message);
      }
      throw error;
    }
  }

  static async getViewDetails(
    projectId: string,
    filePathB64: string,
    branchName?: string
  ): Promise<ViewResponse> {
    try {
      const params = branchName ? { branch: branchName } : {};
      const response = await apiClient.get(`/${projectId}/semantic/view/${filePathB64}`, {
        params
      });
      return response.data;
    } catch (error) {
      if (error instanceof AxiosError && error.response?.data?.message) {
        throw new Error(error.response.data.message);
      }
      throw error;
    }
  }
}

// ── Preagg status ─────────────────────────────────────────────────────────────

type PreaggMeasure = {
  name: string;
  measure_type: string;
};

/** Timestamps are RFC3339 UTC — the server normalizes the manifest's naive
 *  `build_date` before it goes on the wire (`normalize_manifest_timestamp`). */
export type PreaggRollupStatus = {
  view_name: string;
  rollup_name: string;
  /** Whether ANY node has built this rollup — the manifest lists it. This is
   *  the fact that decides whether queries are served from the rollup, since a
   *  node without the file reads the same object from the blob store. */
  is_built: boolean;
  /** Whether the IDE node holds the Parquet on its own disk. A locality
   *  detail, not a capability: `is_built && !has_parquet` still serves. */
  has_parquet: boolean;
  dimensions: string[];
  measures: PreaggMeasure[];
  time_dimension: string | null;
  granularity: string | null;
  /** The rollup's declared refresh cadence (`every 1h`, `sql`), view-level key
   *  included. Present even for a rollup that has never been built. */
  refresh_key: string | null;
  build_date: string | null;
  refresh_key_checked_at: string | null;
  /** When the last rebuild produced ZERO rows, so the rollup's entry and
   *  Parquet were retracted rather than left serving the previous build's
   *  numbers. RFC3339 UTC.
   *
   *  "Empty" and "never built" are otherwise the same row — both `is_built:
   *  false` with no build time — and they are different facts. It is also the
   *  only field that moves after a zero-row rebuild, so a caller waiting on
   *  `build_date` would wait forever. */
  empty_since: string | null;
};

export type PreaggStatusResponse = {
  /** Every rollup the semantic model DECLARES, cached or not — the list comes
   *  from `pre_aggregations:` in the views, not from what happens to be built.
   *  So an empty array really does mean "this workspace declares none". */
  rollups: PreaggRollupStatus[];
  /** Whether a rollup built on another node is readable from here (a blob
   *  bucket is configured). When false, `is_built && !has_parquet` means the
   *  warehouse answers — so the UI must not promise a fast path. */
  blob_reads_available: boolean;
};

export type PreaggRebuildRequest = {
  /** Both or neither: a targeted rebuild names its rollup, everything else
   *  rebuilds the whole declared set. */
  view?: string;
  rollup?: string;
};

export type PreaggRebuildResponse = {
  run_id: string;
  rollups: number;
};

/** Force a rebuild — one rollup, or every declared one. Returns once the work
 *  is submitted; poll `getPreaggStatus` to watch the cache fill in. */
export async function rebuildPreagg(
  workspaceId: string,
  body: PreaggRebuildRequest,
  branchName?: string
): Promise<PreaggRebuildResponse> {
  const params = branchName ? { branch: branchName } : {};
  const { data } = await apiClient.post<PreaggRebuildResponse>(
    `/${workspaceId}/semantic/preagg-rebuild`,
    body,
    { params }
  );
  return data;
}

export async function getPreaggStatus(
  workspaceId: string,
  branchName?: string
): Promise<PreaggStatusResponse> {
  const params = branchName ? { branch: branchName } : {};
  const { data } = await apiClient.get<PreaggStatusResponse>(
    `/${workspaceId}/semantic/preagg-status`,
    { params }
  );
  return data;
}
