// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, waitFor } from "@testing-library/react";
import type { AxiosAdapter } from "axios";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BuilderDialog } from "@/components/BuilderDialog";
import { FileQuickOpen } from "@/components/FileQuickOpen";
import SettingsDialog from "@/components/settings/SettingsDialog";
import { SidebarProvider } from "@/components/ui/shadcn/sidebar";
import { ManageWorkspacesDialog } from "@/components/workspaces/components/ManageWorkspacesDialog";
import { AuthProvider } from "@/contexts/AuthContext";
import { apiClient } from "@/services/api/axios";
import useBuilderDialog from "@/stores/useBuilderDialog";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import type { Workspace } from "@/types/workspace";
import { WorkspaceShell } from "./WorkspaceShell";

/**
 * A customer screen never calls a Factory route.
 *
 * `BuilderDialog` is mounted for every page and asked `GET /{workspace}/modeling`
 * on load; once enforcement is on that is a `403 factory_access_required` per page
 * load, and a toast for each. Rendering what every page mounts, as someone the
 * server will not let into the Factory, and recording the requests it sends is the
 * check that does not depend on remembering which hook is mounted where.
 *
 * The prefixes are the route table's Factory list
 * (`server/factory_access/routes.rs`) at its coarsest: a route the customer list
 * names exactly (`GET /databases`, `GET /status`, …) is not among them.
 */
const FACTORY_PREFIXES = [
  "/files",
  "/modeling",
  "/branches",
  "/repositories",
  "/compile",
  "/tests",
  "/simulations",
  "/secrets",
  "/traces",
  "/metrics",
  "/execution-analytics",
  "/cameras",
  "/camera-packs",
  "/fleet",
  "/world-model",
  "/worktrees",
  "/revision-info",
  "/recent-commits",
  "/semantic/topic",
  "/semantic/view",
  "/semantic/metric-tree",
  "/semantic/world-model",
  "/analytics/coordinator",
  // The working copy's git state and config status, and the setup checks: the
  // Factory's since the apps-first experience, which asks for them only for a
  // Factory user.
  "/git-state",
  "/status",
  "/onboarding-readiness",
  "/onboarding",
  "/analytics/builder"
];

/** Exact customer routes under a Factory prefix above. */
const CUSTOMER_EXCEPTIONS = ["GET /semantic/world-model"];

// An animation library that wants a canvas jsdom does not have; not under test.
vi.mock("@lottiefiles/react-lottie-player", () => ({ Player: () => null }));

vi.mock("@/hooks/api/users/useCurrentUser", () => ({
  default: () => ({ data: { is_owner: false, is_app_admin: false, partner_memberships: [] } })
}));

// `factory_access: false` only ever comes from a server with the apps-first flag on,
// which is also what turns the customer's screens off the Factory's routes.
const authConfig = { billing_enabled: false, apps_first_experience: true, mode: "cloud" } as never;

const requested: string[] = [];

const customerWorkspace = (factory_access: boolean): Workspace =>
  ({
    id: "ws-1",
    name: "Acme",
    factory_access,
    active_branch: { name: "main" },
    default_branch: "main",
    current_user_role: "admin"
  }) as unknown as Workspace;

const pathOf = (url: string | undefined) => (url ?? "").replace(/^\/[^/]+(?=\/)/, "");

function renderWhatEveryPageMounts() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={queryClient}>
      <AuthProvider authConfig={authConfig}>
        <MemoryRouter initialEntries={["/acme/workspaces/ws-1/home"]}>
          <SidebarProvider>
            <BuilderDialog />
            <FileQuickOpen />
            <SettingsDialog />
            <ManageWorkspacesDialog />
            <WorkspaceShell>
              <div />
            </WorkspaceShell>
          </SidebarProvider>
        </MemoryRouter>
      </AuthProvider>
    </QueryClientProvider>
  );
}

beforeEach(() => {
  requested.length = 0;
  const adapter: AxiosAdapter = async (config) => {
    requested.push(`${(config.method ?? "get").toUpperCase()} ${config.url ?? ""}`);
    return { data: [], status: 200, statusText: "OK", headers: {}, config };
  };
  apiClient.defaults.adapter = adapter;
  useCurrentOrg.setState({
    org: { id: "org-1", slug: "acme", name: "Acme" } as never,
    role: "admin"
  });
});

afterEach(() => {
  cleanup();
  useCurrentWorkspace.setState({ workspace: null });
  useBuilderDialog.setState({ isOpen: false });
});

describe("what every page mounts", () => {
  it("sends no request to a Factory route for someone the server does not let in", async () => {
    useCurrentWorkspace.setState({ workspace: customerWorkspace(false) });
    renderWhatEveryPageMounts();
    // Let the queries that would run, run.
    await waitFor(() => expect(requested.length).toBeGreaterThan(0));
    await new Promise((resolve) => setTimeout(resolve, 100));

    const factory = requested.filter((r) => {
      const [method, url] = r.split(" ");
      const path = pathOf(url);
      if (CUSTOMER_EXCEPTIONS.includes(`${method} ${path}`)) return false;
      return FACTORY_PREFIXES.some((p) => path === p || path.startsWith(`${p}/`));
    });
    expect(factory).toEqual([]);
  });

  /// The control: the same screens do ask for the model and the tree when the
  /// server lets the caller in, so the assertion above is not passing because
  /// nothing renders.
  it("does ask for the modeling projects when the caller may use the Factory", async () => {
    useCurrentWorkspace.setState({ workspace: customerWorkspace(true) });
    renderWhatEveryPageMounts();
    await waitFor(() => expect(requested.some((r) => r.includes("/modeling"))).toBe(true));
    // And the config status the banner reads, which a customer is never asked for.
    await waitFor(() => expect(requested.some((r) => r.includes("/status"))).toBe(true));
  });
});
