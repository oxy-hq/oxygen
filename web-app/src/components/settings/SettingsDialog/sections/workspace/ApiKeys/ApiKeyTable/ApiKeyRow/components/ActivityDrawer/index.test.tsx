// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { AxiosError, type AxiosResponse } from "axios";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { TokenEndpoints } from "@/hooks/api/apiKeys/tokenEndpoints";
import type { ApiKey, ApiKeyActivityResponse } from "@/types/apiKey";
import type { TokenSummary } from "@/types/apiToken";
import ActivityDrawer from ".";

vi.setConfig({ testTimeout: 20000 });

type QueryState = {
  data?: ApiKeyActivityResponse;
  isLoading: boolean;
  error: Error | null;
  refetch: () => void;
};
let query: QueryState = { isLoading: true, error: null, refetch: vi.fn() };

// The query hook is the seam: this file is about what each state renders.
vi.mock("@/hooks/api/apiKeys/useApiKeyActivity", () => ({
  API_KEY_ACTIVITY_LIMIT: 100,
  default: () => query
}));

// jsdom has no canvas; the chart's own spec is not under test here.
vi.mock("@/components/Echarts/EChart", () => ({
  default: () => <div data-testid='echart' />
}));

afterEach(cleanup);

const apiKey: ApiKey = {
  id: "k1",
  name: "CI deploy",
  created_at: "2026-01-01T00:00:00Z",
  is_active: true
};

// Never called: the query hook above is mocked. The drawer only passes it through.
const endpoints = {} as TokenEndpoints;

const show = (state: Partial<QueryState>) => {
  query = { isLoading: false, error: null, refetch: vi.fn(), ...state };
  render(<ActivityDrawer token={apiKey} endpoints={endpoints} open onOpenChange={() => {}} />);
};

const httpError = (status: number) =>
  new AxiosError("Request failed", "ERR", undefined, undefined, {
    status,
    data: {}
  } as AxiosResponse);

const today = new Date().toISOString().slice(0, 10);

describe("ActivityDrawer", () => {
  it("shows a skeleton while loading", () => {
    show({ isLoading: true });
    expect(screen.getByTestId("api-key-activity-loading")).toBeInTheDocument();
  });

  it("treats a 404 as 'not available', not as a failure", () => {
    show({ error: httpError(404) });
    expect(screen.getByTestId("api-key-activity-unavailable")).toBeInTheDocument();
    expect(screen.queryByTestId("api-key-activity-error")).not.toBeInTheDocument();
  });

  it("offers a retry for any other failure", () => {
    show({ error: httpError(500) });
    expect(screen.getByTestId("api-key-activity-retry")).toBeInTheDocument();
  });

  it("says never used, no requests and no history for a key with nothing on record", () => {
    show({ data: { events: [], usage: [], last_used: null } });
    expect(screen.getByTestId("api-key-activity-last-used")).toHaveTextContent("Never used.");
    expect(screen.getByTestId("api-key-activity-usage")).toHaveTextContent(
      "No requests in the last 30 days."
    );
    expect(screen.queryByTestId("echart")).not.toBeInTheDocument();
  });

  it("renders last use, totals, and an extension's old → new expiry", () => {
    show({
      data: {
        last_used: {
          at: new Date().toISOString(),
          ip: "203.0.113.4",
          user_agent: "oxyc/0.5.0",
          route: "/api/{workspace_id}/sql/query"
        },
        usage: [{ day: today, requests: 120, errors_4xx: 3, errors_5xx: 1 }],
        events: [
          {
            id: "e1",
            created_at: new Date().toISOString(),
            actor_email: "owner@example.com",
            actor_type: "user",
            action: "token.extended",
            org_id: null,
            workspace_id: null,
            partner_id: null,
            target_type: "api_key",
            target_id: "k1",
            target_label: "CI deploy",
            outcome: "success",
            reason: null,
            via_global_override: false,
            metadata: { old_expires_at: "2026-10-03T12:00:00Z", new_expires_at: null }
          },
          {
            id: "e2",
            created_at: new Date().toISOString(),
            actor_email: "owner@example.com",
            actor_type: "api_key",
            action: "secret.updated",
            org_id: null,
            workspace_id: null,
            partner_id: null,
            target_type: "secret",
            target_id: "s1",
            target_label: "WAREHOUSE_PASSWORD",
            outcome: "failure",
            reason: "denied",
            via_global_override: false
          }
        ]
      }
    });
    expect(screen.getByTestId("api-key-activity-last-used")).toHaveTextContent("203.0.113.4");
    expect(screen.getByTestId("api-key-activity-usage-ok")).toHaveTextContent("116");
    expect(screen.getByTestId("api-key-activity-usage-4xx")).toHaveTextContent("3");
    expect(screen.getByTestId("echart")).toBeInTheDocument();
    expect(screen.getByTestId("api-key-activity-expiry-change")).toHaveTextContent(
      "Oct 3, 2026→changed tono expiry"
    );
    const action = screen.getByTestId("api-key-activity-action-item");
    expect(action).toHaveTextContent("secret.updated");
    expect(action).toHaveTextContent("WAREHOUSE_PASSWORD");
    expect(action).toHaveTextContent("failed");
  });

  describe("what it calls the thing", () => {
    const empty = { events: [], usage: [], last_used: null };
    const showFor = (token: TokenSummary, state: Partial<QueryState>) => {
      query = { isLoading: false, error: null, refetch: vi.fn(), ...state };
      render(<ActivityDrawer token={token} endpoints={endpoints} open onOpenChange={() => {}} />);
      return screen.getByTestId("api-key-activity-drawer");
    };

    // A row from the legacy routes carries no `kind`; an org's inventory stamps `legacy_key`.
    it.each([
      ["no kind", apiKey],
      ["kind legacy_key", { ...apiKey, kind: "legacy_key" as const }]
    ])("calls a legacy row (%s) a legacy API key, never a token", (_label, token) => {
      const drawer = showFor(token, { data: empty });
      expect(drawer).toHaveTextContent("No changes recorded for this legacy API key yet.");
      expect(drawer).toHaveTextContent("Actions with this legacy API key");
      expect(drawer).not.toHaveTextContent(/token/i);
      cleanup();

      expect(showFor(token, { error: httpError(404) })).toHaveTextContent(
        "Activity isn't available for this legacy API key"
      );
    });

    it("calls a personal access token a token, never a key", () => {
      const token = { ...apiKey, name: "laptop", kind: "personal" as const };
      const drawer = showFor(token, { data: empty });
      expect(drawer).toHaveTextContent("No changes recorded for this token yet.");
      expect(drawer).toHaveTextContent("Actions with this token");
      expect(drawer).not.toHaveTextContent(/\bkey\b/i);
      cleanup();

      expect(showFor(token, { error: httpError(404) })).toHaveTextContent(
        "Activity isn't available for this token"
      );
    });
  });
});
