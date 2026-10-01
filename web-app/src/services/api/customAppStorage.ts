import type {
  FleetStorageResponse,
  StorageBrowseResponse,
  StorageHistoryResponse,
  StorageSweepStarted
} from "@/types/apps";
import { apiClient } from "./axios";

/**
 * Staff-facing custom-app storage surface. Two data sources on purpose:
 * `fleet` reads the `app_storage_usage` rollup (ranking every app cannot mean
 * walking every S3 prefix per request), while `browse` reads S3 live (the
 * rollup holds no per-object rows, and an operator investigating right now
 * needs current truth rather than a number up to a sweep old).
 *
 * See `internal-docs/customer-apps-functions.md` ("Quotas, usage & the admin Storage tab").
 */
export const CustomAppStorageService = {
  /**
   * Daily totals for the usage chart. Fleet-wide unless `appId` narrows it.
   * Each point is the value HELD at that day's end, not that day's writes.
   */
  history: (days: number, appId?: string) =>
    apiClient
      .get<StorageHistoryResponse>("/customer-apps/storage/history", {
        params: { days, appId }
      })
      .then((r) => r.data),

  fleet: (sort?: "bytes" | "growth" | "untagged", limit?: number) =>
    apiClient
      .get<FleetStorageResponse>("/customer-apps/storage", { params: { sort, limit } })
      .then((r) => r.data),

  browse: (appId: string, opts?: { prefix?: string; cursor?: string; limit?: number }) =>
    apiClient
      .get<StorageBrowseResponse>(`/customer-apps/${appId}/storage/objects`, { params: opts })
      .then((r) => r.data),

  deleteObjects: (appId: string, keys: string[]) =>
    apiClient
      .post<{ deleted: number }>(`/customer-apps/${appId}/storage/delete`, { keys })
      .then((r) => r.data),

  /**
   * Kick off a re-measure so a stale number is actionable, not just visible.
   *
   * Returns 202 as soon as the sweep starts — it walks S3 for up to 24 apps and
   * would otherwise hold the request for minutes. Poll `fleet` for the result;
   * each row carries its own `measuredAt`. A 409 means one is already running.
   */
  sweep: () =>
    apiClient.post<StorageSweepStarted>("/customer-apps/storage/sweep").then((r) => r.data)
};
