import { AxiosError } from "axios";
import type {
  AnomalyStatus,
  BulkUpdateStatusResponse,
  ListAnomaliesResponse,
  ListMonitorsResponse,
  MonitorPreview,
  MonitorSelector,
  ScanAnomaliesResponse,
  StatusWriteGroup
} from "@/types/metricAnomalies";
import type { ExplainResult } from "@/types/metricTree";
import { apiClient } from "./axios";

/**
 * Surface the server's own words.
 *
 * These handlers answer with a **plain-text** body — `(StatusCode, String)` on
 * the Rust side — so `data` is the message itself, not `{ message }`. Reading
 * only the JSON shape threw the AxiosError instead, and the UI rendered
 * "Request failed with status code 500" over the one line that said anything:
 * the scan's `ScanError` chain, or the 400s that tell a caller exactly what to
 * do ("that selection covers N buckets… narrow it and repeat").
 *
 * Both shapes are handled because this client is one of several and the JSON
 * envelope is the house style elsewhere.
 *
 * A plain-text body is only *our* handlers' shape, though. A 502 from a load
 * balancer or CDN answers with an HTML error page, which is also a string — so
 * that is skipped and the AxiosError's own "Request failed with status code
 * 502" is thrown instead, and anything else is capped: a toast is one line, and
 * an unbounded body swamps it either way.
 */
const MAX_MESSAGE = 300;

function rethrow(error: unknown): never {
  if (error instanceof AxiosError) {
    const body = error.response?.data;
    const raw = typeof body === "string" ? body.trim() : body?.message;
    const message = typeof raw === "string" && !raw.startsWith("<") ? raw : undefined;
    if (message) {
      throw new Error(
        message.length > MAX_MESSAGE ? `${message.slice(0, MAX_MESSAGE - 1)}…` : message
      );
    }
  }
  throw error;
}

/** Client for the `/semantic/anomalies*` endpoints. */
export class MetricAnomaliesService {
  /**
   * List one page of anomalies for the workspace, optionally filtered by status.
   *
   * `order="recent"` asks for latest-first (`detected_at DESC`); the default
   * ranks worst-first by event severity for the Insights Inbox.
   *
   * `limit`/`offset` count **events** under the default ranking (every bucket
   * of a returned event comes back, so the row count is larger) and rows under
   * `order="recent"`. `total` in the response uses the same unit, so paging
   * maths never has to guess at the grouping.
   */
  static async list(
    projectId: string,
    status?: AnomalyStatus,
    order?: "recent",
    page?: { limit: number; offset: number }
  ): Promise<ListAnomaliesResponse> {
    try {
      const response = await apiClient.get<ListAnomaliesResponse>(
        `/${projectId}/semantic/anomalies`,
        {
          params: {
            ...(status ? { status } : {}),
            ...(order ? { order } : {}),
            ...(page ? { limit: page.limit, offset: page.offset } : {})
          }
        }
      );
      return response.data;
    } catch (error) {
      rethrow(error);
    }
  }

  /** Run every `.monitor.yml` entry once and upsert detected anomalies.
   *  `asOf` (YYYY-MM-DD) overrides the "now" reference the detector uses —
   *  handy for demos against datasets that don't include the current date. */
  static async scan(projectId: string, asOf?: string): Promise<ScanAnomaliesResponse> {
    try {
      const response = await apiClient.post<ScanAnomaliesResponse>(
        `/${projectId}/semantic/anomalies/scan`,
        null,
        { params: asOf ? { as_of: asOf } : {} }
      );
      return response.data;
    } catch (error) {
      rethrow(error);
    }
  }

  /** Dry-run one `.monitor.yml` entry: what the next scan would flag, with
   *  nothing written — no inbox rows, no run, no Slack post. */
  static async previewMonitor(
    projectId: string,
    selector: MonitorSelector
  ): Promise<MonitorPreview> {
    try {
      const response = await apiClient.post<MonitorPreview>(
        `/${projectId}/semantic/anomalies/preview`,
        selector
      );
      return response.data;
    } catch (error) {
      rethrow(error);
    }
  }

  /** Run (or return cached) airlayer explain for a single anomaly. Cache
   *  lives on the row itself, so reopening the same anomaly across page
   *  refreshes returns instantly. `refresh=true` busts the cache. */
  static async explain(
    projectId: string,
    anomalyId: string,
    refresh = false
  ): Promise<ExplainResult> {
    try {
      const response = await apiClient.post<ExplainResult>(
        `/${projectId}/semantic/anomalies/${anomalyId}/explain`,
        null,
        { params: refresh ? { refresh: "true" } : {} }
      );
      return response.data;
    } catch (error) {
      rethrow(error);
    }
  }

  /** List all entries from `.monitor.yml` for the workspace. Returns an
   *  empty array when no file is configured; returns an error when the
   *  file exists but fails to parse. */
  static async listMonitors(projectId: string): Promise<ListMonitorsResponse> {
    try {
      const response = await apiClient.get<ListMonitorsResponse>(`/${projectId}/semantic/monitors`);
      // `coverage` is absent from responses served before it shipped, and from
      // workspaces that have never been scanned. `notify` is absent whenever
      // the file has no such block. Each field is carried over by name, so one
      // left off this list is dropped without a type error — which is how the
      // delivery note came to say "only in this inbox" for every workspace.
      return {
        monitors: response.data.monitors ?? [],
        coverage: response.data.coverage ?? [],
        notify: response.data.notify ?? null
      };
    } catch (error) {
      rethrow(error);
    }
  }

  /**
   * Move anomalies between new / acknowledged / dismissed — one request for
   * the whole set.
   *
   * Targets are events (`eventIds`) wherever there is one, and bare row `ids`
   * only for pre-event rows that have no event to name. That split matters:
   * a list response caps how many buckets it returns per event, so sending the
   * bucket ids we happen to hold would leave the tail of a long chain `new`
   * while the toast claimed success.
   *
   * `onlyStatuses` bounds which of an event's buckets may move: the live ones
   * always, and the dismissed ones only when the row acted on was itself
   * dismissed. Acking a New row must not resurrect what the user retired.
   * `updated` counts the rows the server actually wrote, so zero means the
   * selection was stale.
   */
  static async updateStatus(
    projectId: string,
    target: StatusWriteGroup,
    status: AnomalyStatus
  ): Promise<BulkUpdateStatusResponse> {
    try {
      const response = await apiClient.post<BulkUpdateStatusResponse>(
        `/${projectId}/semantic/anomalies/status`,
        {
          ids: target.ids,
          event_ids: target.eventIds,
          only_statuses: target.onlyStatuses,
          status
        }
      );
      return response.data;
    } catch (error) {
      rethrow(error);
    }
  }
}
