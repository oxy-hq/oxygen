import { apiClient } from "./axios";

/**
 * Admin → Metrics' per-org LLM usage, read by the org-360's cost card.
 *
 * The cross-tenant overview (`GET /admin/metrics/llm-usage`) and its shapes
 * left with their only caller when Admin → Observability replaced
 * `LlmCostSection`. The endpoint is still served; nothing in the web app asks
 * for it, and the fleet list asks `/admin/observability/fleet` instead, which
 * covers every org rather than a top-10 leaderboard.
 */

interface UsageTotals {
  cost_usd: number;
  input_tokens: number;
  output_tokens: number;
  cache_creation_tokens: number;
  cache_read_tokens: number;
  run_count: number;
  priced_run_count: number;
}

export interface DayCost {
  day: string;
  cost_usd: number;
  input_tokens: number;
  output_tokens: number;
  run_count: number;
}

/**
 * Usage scoped to a single org — correct for any tenant, with the daily
 * series behind the trend. The cross-tenant overview it was distinguished
 * from kept only a top-10 leaderboard, so an org outside it read as zero;
 * Admin → Observability's fleet list has no such cut.
 */
export interface OrgUsageDetail {
  window_days: number;
  total: UsageTotals;
  by_day: DayCost[];
}

export const AdminMetricsService = {
  async orgUsage(orgId: string, days = 30): Promise<OrgUsageDetail> {
    const res = await apiClient.get<OrgUsageDetail>(`/admin/metrics/orgs/${orgId}/llm-usage`, {
      params: { days }
    });
    return res.data;
  }
};
