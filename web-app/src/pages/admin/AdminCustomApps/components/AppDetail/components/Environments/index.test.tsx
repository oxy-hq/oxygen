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
    error: null,
    recut_queued: false
  }
};

const renderPanel = () => {
  // The hook keeps its own retry count; no delay keeps a failing fetch quick.
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, retryDelay: 0 } } });
  render(
    <QueryClientProvider client={client}>
      <Environments appId='app-1' />
    </QueryClientProvider>
  );
  return client;
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

  it("prints a failed branch's reason and offers Delete again on a sandbox being deleted", async () => {
    vi.mocked(CustomAppsService.listEnvironments).mockResolvedValue([
      base,
      {
        ...sandbox,
        status: "deleting",
        oltp_branch: {
          ...(sandbox.oltp_branch as NonNullable<AppEnvironment["oltp_branch"]>),
          status: "failed",
          error: "the provider refused the cut"
        }
      }
    ]);
    renderPanel();
    const error = await screen.findByTestId("admin-app-env-oltp-error-dev-pay");
    expect(error.textContent).toBe("the provider refused the cut");
    const del = screen.getByTestId("admin-app-env-delete-dev-pay") as HTMLButtonElement;
    expect(del.disabled).toBe(false);
    expect(del.textContent).toBe("Retry delete");
  });

  it("says a queued re-cut is queued, not ready", async () => {
    vi.mocked(CustomAppsService.listEnvironments).mockResolvedValue([
      base,
      {
        ...sandbox,
        oltp_branch: {
          ...(sandbox.oltp_branch as NonNullable<AppEnvironment["oltp_branch"]>),
          status: "ready",
          recut_queued: true
        }
      }
    ]);
    renderPanel();
    expect((await screen.findByTestId("admin-app-env-oltp-dev-pay")).textContent).toBe(
      "oltp branch re-cut queued"
    );
  });

  it("shows the server's words and a Retry when the list cannot load", async () => {
    vi.mocked(CustomAppsService.listEnvironments).mockRejectedValue(new Error("database is down"));
    renderPanel();
    expect((await screen.findByTestId("admin-async-error-detail")).textContent).toBe(
      "database is down"
    );
    expect(screen.getByTestId("admin-async-retry")).toBeTruthy();
  });

  it("keeps the list it has when a refetch fails", async () => {
    const client = renderPanel();
    await screen.findByTestId("admin-app-env-row-dev-pay");
    vi.mocked(CustomAppsService.listEnvironments).mockRejectedValue(new Error("database is down"));
    await client.refetchQueries();
    await waitFor(() =>
      expect(client.getQueryState(["customApps", "environments", "app-1"])?.status).toBe("error")
    );
    expect(screen.getByTestId("admin-app-env-row-dev-pay")).toBeTruthy();
    expect(screen.queryByTestId("admin-async-error")).toBeNull();
  });

  it("refreshes the Secrets pane after a secrets reset", async () => {
    const client = renderPanel();
    const key = ["customApps", "secrets", "app-1", "dev-pay"];
    client.setQueryData(key, { keys: ["STRIPE_KEY"] });
    fireEvent.click(await screen.findByTestId("admin-app-env-details-dev-pay"));
    fireEvent.click(await screen.findByTestId("admin-app-env-reset-secrets"));
    const confirm = await screen.findByTestId("admin-app-env-reset-confirm");
    fireEvent.click(within(confirm).getByRole("button", { name: "Reset" }));
    await waitFor(() => expect(client.getQueryState(key)?.isInvalidated).toBe(true));
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
