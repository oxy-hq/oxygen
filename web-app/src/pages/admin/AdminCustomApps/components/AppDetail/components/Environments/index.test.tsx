// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { type AppEnvironment, CustomAppsService } from "@/services/api/customApps";

vi.mock("@/services/api/customApps", () => ({
  CustomAppsService: {
    listEnvironments: vi.fn(),
    createEnvironment: vi.fn(),
    deleteEnvironment: vi.fn(),
    environmentOutbox: vi.fn(),
    resetEnvironmentPlanes: vi.fn()
  }
}));

import { Environments } from "./index";

const base: AppEnvironment = {
  name: "production",
  kind: "production",
  status: "active",
  build_id: null,
  build_uuid: null,
  semantic_revision_id: null,
  owner: null,
  created_at: "2026-11-01T00:00:00Z",
  updated_at: "2026-11-01T00:00:00Z",
  last_activity_at: null,
  expires_at: null,
  url: null,
  oltp_schema: null,
  parent: null,
  fork_at: null,
  planes: null,
  planes_error: null,
  planes_set: null,
  oltp_branch: null
};

const sandbox: AppEnvironment = {
  ...base,
  name: "dev-pay",
  kind: "dev",
  parent: "staging",
  fork_at: "2026-11-02T00:00:00Z",
  planes: {
    oltp: "branch",
    airhouse: "branch",
    storage: "branch",
    secrets: "empty",
    messages: "capture",
    fetch: "allow"
  },
  planes_set: {
    oltp: "branch",
    secrets: "empty",
    messages: "capture",
    fetch: { mode: "allow", hosts: ["connect.squareupsandbox.com"] }
  },
  oltp_branch: {
    mode: "branch",
    status: "migrating",
    fork_at: "2026-11-02T00:00:00Z",
    expires_at: "2026-12-02T00:00:00Z",
    error: null
  }
};

const renderPanel = () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <Environments appId='app-1' />
    </QueryClientProvider>
  );
};

beforeEach(() => {
  vi.mocked(CustomAppsService.listEnvironments).mockResolvedValue([base, sandbox]);
  vi.mocked(CustomAppsService.environmentOutbox).mockResolvedValue({
    messages: [
      {
        kind: "email",
        to: ["buyer@example.com"],
        subject: "Your receipt",
        body: "<b>Thanks</b>",
        invocation_id: "inv-1",
        function: "checkout",
        captured_at: "2026-11-02T01:00:00Z"
      }
    ],
    has_more: false
  });
  vi.mocked(CustomAppsService.resetEnvironmentPlanes).mockResolvedValue({
    name: "dev-pay",
    planes: [{ plane: "storage", status: "done", run_id: null }]
  });
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("Environments", () => {
  it("shows a sandbox's parent, chosen planes, OLTP branch status and deletion date", async () => {
    renderPanel();
    const branch = await screen.findByTestId("admin-app-env-branch-dev-pay");
    expect(branch.textContent).toContain("from staging");
    expect(screen.getByTestId("admin-app-env-plane-dev-pay-oltp").textContent).toBe("oltp=branch");
    expect(screen.getByTestId("admin-app-env-plane-dev-pay-fetch").textContent).toBe("fetch=allow");
    expect(screen.queryByTestId("admin-app-env-plane-dev-pay-airhouse")).toBeNull();
    expect(screen.getByTestId("admin-app-env-oltp-dev-pay").textContent).toBe(
      "oltp branch migrating"
    );
    expect(screen.getByTestId("admin-app-env-deleted-dev-pay").textContent).toContain(
      "30 days of copied data"
    );
    expect(screen.queryByTestId("admin-app-env-details-production")).toBeNull();
  });

  it("opens the drawer on every plane, and shows a captured email as text", async () => {
    renderPanel();
    fireEvent.click(await screen.findByTestId("admin-app-env-details-dev-pay"));
    const drawer = await screen.findByTestId("admin-app-env-drawer");
    const fetchRow = within(drawer).getByTestId("admin-app-env-plane-row-fetch");
    expect(fetchRow.textContent).toContain("allow → connect.squareupsandbox.com");
    expect(within(drawer).getByTestId("admin-app-env-reset-oltp")).toBeTruthy();
    expect(within(drawer).queryByTestId("admin-app-env-reset-reads")).toBeNull();

    fireEvent.mouseDown(within(drawer).getByTestId("admin-app-env-tab-outbox"));
    fireEvent.click(within(drawer).getByTestId("admin-app-env-tab-outbox"));
    const item = await within(drawer).findByTestId("admin-app-env-outbox-item");
    expect(item.textContent).toContain("Your receipt");
    expect(item.textContent).toContain("<b>Thanks</b>");
    expect(item.querySelector("b")).toBeNull();
  });

  it("resets one plane only after its confirm", async () => {
    renderPanel();
    fireEvent.click(await screen.findByTestId("admin-app-env-details-dev-pay"));
    fireEvent.click(await screen.findByTestId("admin-app-env-reset-storage"));
    expect(CustomAppsService.resetEnvironmentPlanes).not.toHaveBeenCalled();
    const confirm = await screen.findByTestId("admin-app-env-reset-confirm");
    fireEvent.click(within(confirm).getByRole("button", { name: "Reset" }));
    await waitFor(() =>
      expect(CustomAppsService.resetEnvironmentPlanes).toHaveBeenCalledWith("app-1", "dev-pay", [
        "storage"
      ])
    );
  });
});
