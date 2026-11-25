// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { AxiosError, AxiosHeaders } from "axios";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { UserService } from "@/services/api";
import { CustomAppsService } from "@/services/api/customApps";
import type { RecentTraces } from "@/types/appRecentTraces";
import type { AppFunctionSummary, AppSecrets } from "@/types/apps";

vi.mock("@/services/api/customApps", () => ({
  CustomAppsService: {
    listFunctions: vi.fn(),
    listRecentTraces: vi.fn(),
    listSecrets: vi.fn()
  }
}));
// Who is looking decides whether the Data flow link shows.
vi.mock("@/services/api", () => ({
  UserService: { getCurrentUser: vi.fn() }
}));

import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import { AppMapPanel } from "./index";

const sync: AppFunctionSummary = {
  name: "sync",
  route: true,
  schedule: "0 * * * *",
  timezone: null,
  airway: false,
  timeout_seconds: null,
  retries: null,
  secrets_write: false,
  destinations: ["analytics"]
};

const traces: RecentTraces = {
  environment: "staging",
  per_function: 5,
  retention_days: 3,
  functions: [
    {
      function: "sync",
      traces: [
        {
          invocation_id: "i1",
          created_at: "2026-11-17T10:00:00Z",
          mode: "schedule",
          failed: true,
          total_calls: 3,
          dropped: 0,
          touches: [
            {
              family: "query",
              op: "query",
              verb: "SELECT",
              target: "orders",
              status: "ok",
              calls: 2
            },
            {
              family: "fetch",
              op: "fetch",
              verb: "POST",
              target: "api.vendor.com",
              status: "error",
              error_type: "upstream_5xx",
              calls: 1
            }
          ]
        }
      ]
    }
  ]
};

const secrets: AppSecrets = {
  app_id: "app-1",
  app_slug: "ops",
  app_name: "Ops",
  environment: "staging",
  entries: [],
  missing_required: 0
};

const axiosError = (status: number, message: string) => {
  const config = { headers: new AxiosHeaders() };
  return new AxiosError("Request failed", String(status), config, null, {
    status,
    statusText: "",
    headers: {},
    config,
    data: { error: "non_production_refused", message }
  });
};

const renderPanel = (onOpenFunction = vi.fn()) => {
  render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      <MemoryRouter>
        <AppMapPanel
          appId='app-1'
          orgId='org-1'
          environment='staging'
          scope={{ label: "Staging", warn: true }}
          selected={null}
          onOpenFunction={onOpenFunction}
        />
      </MemoryRouter>
    </QueryClientProvider>
  );
  return onOpenFunction;
};

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("AppMapPanel", () => {
  it("reads the console's environment and lists the map where there is no room to draw it", async () => {
    vi.mocked(CustomAppsService.listFunctions).mockResolvedValue([sync]);
    vi.mocked(CustomAppsService.listRecentTraces).mockResolvedValue(traces);
    vi.mocked(CustomAppsService.listSecrets).mockResolvedValue(secrets);
    const onOpen = renderPanel();

    // jsdom lays nothing out, so the column is 0px wide: the list stands in.
    const row = await screen.findByTestId("admin-app-map-list-sync");
    await waitFor(() => expect(within(row).getByText(/api\.vendor\.com/)).toBeTruthy());
    expect(CustomAppsService.listRecentTraces).toHaveBeenCalledWith("app-1", "staging");
    expect(screen.getByTestId("apps-console-panel-app-map-scope").textContent).toBe("Staging");

    expect(within(row).getByText(/Started by:/).textContent).toContain("Schedule 0 * * * *");
    // The failed host is the one line in the danger tone.
    const failed = within(row)
      .getByText(/api\.vendor\.com/)
      .closest("li");
    expect(failed?.className).toContain(ADMIN_TONE.danger.text);
    const fine = within(row)
      .getByText(/orders/)
      .closest("li");
    expect(fine?.className).not.toContain(ADMIN_TONE.danger.text);
    expect(within(row).getByText(/declared destination/)).toBeTruthy();

    expect(screen.getByTestId("admin-app-map-freshness").textContent).toContain(
      "last 5 recorded runs: 1 run"
    );

    fireEvent.click(within(row).getByRole("button", { name: "sync" }));
    expect(onOpen).toHaveBeenCalledWith("sync");
  });

  it("keeps the manifest's side when the runs cannot be read, and says why", async () => {
    vi.mocked(CustomAppsService.listFunctions).mockResolvedValue([sync]);
    vi.mocked(CustomAppsService.listRecentTraces).mockRejectedValue(
      axiosError(403, "you may not open this app's non-production environments")
    );
    vi.mocked(CustomAppsService.listSecrets).mockResolvedValue(secrets);
    renderPanel();

    const error = await screen.findByTestId("admin-app-map-traces-error");
    expect(error.textContent).toContain("non-production environments");
    const row = screen.getByTestId("admin-app-map-list-sync");
    expect(within(row).getByText(/analytics/)).toBeTruthy();
    expect(within(row).getByText(/runs not read yet/)).toBeTruthy();
  });

  it("links to the org's Data flow, focused on the app, for whoever may open it", async () => {
    vi.mocked(CustomAppsService.listFunctions).mockResolvedValue([sync]);
    vi.mocked(CustomAppsService.listRecentTraces).mockResolvedValue(traces);
    vi.mocked(CustomAppsService.listSecrets).mockResolvedValue(secrets);
    vi.mocked(UserService.getCurrentUser).mockResolvedValue({
      is_owner: false,
      platform_capabilities: ["operate_platform"]
    } as unknown as Awaited<ReturnType<typeof UserService.getCurrentUser>>);
    renderPanel();

    const link = await screen.findByTestId("admin-app-map-data-flow-link");
    expect(link.getAttribute("href")).toBe("/admin/data-flow?org=org-1&focus=app%3Aapp-1");
  });

  it("offers no Data flow link to an App Operator, whom the page would refuse", async () => {
    vi.mocked(CustomAppsService.listFunctions).mockResolvedValue([sync]);
    vi.mocked(CustomAppsService.listRecentTraces).mockResolvedValue(traces);
    vi.mocked(CustomAppsService.listSecrets).mockResolvedValue(secrets);
    vi.mocked(UserService.getCurrentUser).mockResolvedValue({
      is_owner: false,
      platform_capabilities: ["manage_apps"]
    } as unknown as Awaited<ReturnType<typeof UserService.getCurrentUser>>);
    renderPanel();

    await screen.findByTestId("admin-app-map-list-sync");
    await waitFor(() => expect(UserService.getCurrentUser).toHaveBeenCalled());
    expect(screen.queryByTestId("admin-app-map-data-flow-link")).toBeNull();
  });

  it("says there is nothing to map for a build with no functions", async () => {
    vi.mocked(CustomAppsService.listFunctions).mockResolvedValue([]);
    vi.mocked(CustomAppsService.listRecentTraces).mockResolvedValue({ ...traces, functions: [] });
    vi.mocked(CustomAppsService.listSecrets).mockResolvedValue(secrets);
    renderPanel();
    expect(await screen.findByTestId("admin-app-map-empty")).toBeTruthy();
  });
});
