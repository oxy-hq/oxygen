// @vitest-environment jsdom

/**
 * Which surfaces put `?branch=` on a request, pinned on the wire.
 *
 * The server sends EVERY request carrying a non-empty `?branch=` to the one
 * node that owns the workspace files (`role_middleware::escalate_for_branch`),
 * so the param decides where a request can be served, not only what it reads.
 * The IDE needs that. The coordinator, observability and the camera fleet are
 * mounted under `/ide` too and do not: what they show exists once per
 * workspace, and a request carrying the hint fails whenever that node is down.
 *
 * End to end on the client, as in `contexts/PreviewPinRequests.test.tsx`: the
 * real IDE shell, the real `useCurrentWorkspaceBranch`, real hooks and
 * services through the real `apiClient` — only the network adapter is swapped
 * for one that records what would have gone on the wire. A branch IS selected
 * in the IDE in every case below; the question is who sends it.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import type { AxiosAdapter, InternalAxiosRequestConfig } from "axios";
import { type ComponentType, useEffect } from "react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  ideBranch: "feat/x" as string | undefined,
  Header: null as ComponentType | null
}));

// The shell's chrome is the IDE's and pulls in Monaco and git; stand-ins keep
// the shell itself — the thing under test — real.
vi.mock("./Header", async () => {
  const { createElement } = await import("react");
  return { default: () => (mocks.Header ? createElement(mocks.Header) : null) };
});
vi.mock("./Sidebar", () => ({ default: () => null }));
vi.mock("@/components/ProjectStatus", () => ({ default: () => null }));
vi.mock("@/components/ui/shadcn/sidebar-context", () => ({
  default: () => ({ isMobile: false })
}));
vi.mock("@/stores/useCurrentWorkspace", () => ({
  default: () => ({
    workspace: {
      id: "ws-1",
      active_branch: { name: "main" },
      default_branch: "main",
      protected_branches: ["main"]
    }
  })
}));
vi.mock("@/stores/useIdeBranch", () => ({
  default: () => ({ getCurrentBranch: () => mocks.ideBranch })
}));
vi.mock("sonner", () => ({ toast: { error: vi.fn(), success: vi.fn() } }));

import useAgents from "@/hooks/api/agents/useAgents";
import useFileTree from "@/hooks/api/files/useFileTree";
import { useScheduleAgents } from "@/hooks/api/schedules/useSchedules";
import useCurrentWorkspaceBranch from "@/hooks/useCurrentWorkspaceBranch";
import ROUTES from "@/libs/utils/routes";
import { AgentService } from "@/services/api/agents";
import { apiClient } from "@/services/api/axios";
import { ArtifactService } from "@/services/api/misc";
import Ide from "./index";

const IDE = ROUTES.ORG("acme").WORKSPACE("ws-1").IDE;
/** Local mode: no org, and the workspace is the root. */
const LOCAL_IDE = ROUTES.ORG("").WORKSPACE("ws-1").IDE;

interface Sent {
  path: string;
  /** The `branch` query value, however the call site attached it; null when absent. */
  branch: string | null;
}

const AGENTS = [
  { name: "sales", path: "agents/sales.agentic.yml", public: true },
  { name: "nightly", path: "agents/nightly.aw.yml", public: true }
];

let sent: Sent[] = [];
const recordingAdapter: AxiosAdapter = (config: InternalAxiosRequestConfig) => {
  // `getUri` folds `params` into the URL, so a branch in either place is seen.
  const url = new URL(apiClient.getUri(config), "http://oxy.test");
  const path = url.pathname.slice(url.pathname.indexOf("/ws-1"));
  sent.push({ path, branch: url.searchParams.get("branch") });
  const data = path === "/ws-1/agents" ? AGENTS : {};
  return Promise.resolve({ data, status: 200, statusText: "OK", headers: {}, config });
};

let client: QueryClient;

/**
 * The shape `App.tsx` mounts the shell in: a splat route whose element renders
 * its own `<Routes>`, with `ide` a route inside those. The shell finds its
 * section relative to its own path, so the nesting is part of what is tested.
 */
function Workspace({ Surface }: { Surface: ComponentType }) {
  return (
    <Routes>
      <Route path='ide' element={<Ide />}>
        <Route path='*' element={<Surface />} />
      </Route>
    </Routes>
  );
}

function renderAt(path: string, Surface: ComponentType) {
  const workspace = <Workspace Surface={Surface} />;
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[path]}>
        <Routes>
          <Route path='/:orgSlug'>
            <Route path='workspaces/:wsId/*' element={workspace} />
          </Route>
          {/* Local mode mounts the same workspace at the root. */}
          <Route path='/*' element={workspace} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>
  );
}

const requestsTo = (path: string) => sent.filter((s) => s.path === path);
const firstRequestTo = async (path: string) => {
  await waitFor(() => expect(requestsTo(path)).not.toHaveLength(0));
  return requestsTo(path)[0];
};

/** A surface doing what most real ones do: hand `branchName` to its service. */
function ListsAgents() {
  const { branchName } = useCurrentWorkspaceBranch();
  useEffect(() => {
    void AgentService.listAgents("ws-1", branchName);
  }, [branchName]);
  return null;
}

describe("the branch hint, by section of /ide", () => {
  const originalAdapter = apiClient.defaults.adapter;
  beforeEach(() => {
    mocks.ideBranch = "feat/x";
    mocks.Header = null;
    client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    sent = [];
    apiClient.defaults.adapter = recordingAdapter;
  });
  afterEach(() => {
    cleanup();
    apiClient.defaults.adapter = originalAdapter;
  });

  it.each([
    ["files", IDE.FILES.ROOT],
    ["a file", IDE.FILES.FILE("YWdlbnRzL3NhbGVz")],
    ["database", IDE.DATABASE.ROOT],
    ["modeling", IDE.MODELING.ROOT],
    ["tests", IDE.TESTS.ROOT],
    ["semantic model", IDE.SEMANTIC.ROOT],
    ["world model", IDE.WORLD_MODEL.ROOT]
  ])("%s sends the branch selected in the IDE", async (_section, path) => {
    renderAt(path, ListsAgents);
    expect(await firstRequestTo("/ws-1/agents")).toEqual({
      path: "/ws-1/agents",
      branch: "feat/x"
    });
  });

  it.each([
    ["coordinator", IDE.COORDINATOR.ROOT],
    ["coordinator jobs", IDE.COORDINATOR.JOBS],
    ["a coordinator run", IDE.COORDINATOR.RUN_DETAIL("run-1")],
    ["observability traces", IDE.OBSERVABILITY.TRACES],
    ["observability clusters", IDE.OBSERVABILITY.CLUSTERS],
    ["camera fleet", IDE.EDGE.ROOT],
    ["camera fleet devices", IDE.EDGE.DEVICES]
  ])("%s sends none, whatever branch the IDE has selected", async (_section, path) => {
    renderAt(path, ListsAgents);
    expect(await firstRequestTo("/ws-1/agents")).toEqual({ path: "/ws-1/agents", branch: null });
  });

  it.each([
    ["files", LOCAL_IDE.FILES.ROOT, "feat/x"],
    ["coordinator jobs", LOCAL_IDE.COORDINATOR.JOBS, null],
    ["observability traces", LOCAL_IDE.OBSERVABILITY.TRACES, null]
  ])("in local mode, where /ide sits at the root, %s sends %s", async (_section, path, branch) => {
    renderAt(path, ListsAgents);
    expect(await firstRequestTo("/ws-1/agents")).toEqual({ path: "/ws-1/agents", branch });
  });

  it("the file tree — an IDE read — still names its branch", async () => {
    renderAt(IDE.FILES.ROOT, function FileTree() {
      useFileTree();
      return null;
    });
    expect(await firstRequestTo("/ws-1/files")).toEqual({ path: "/ws-1/files", branch: "feat/x" });
  });

  it("the schedule dialog's agent picker (coordinator) sends no branch", async () => {
    renderAt(IDE.COORDINATOR.JOBS, function AgentPicker() {
      const { data } = useScheduleAgents();
      return <span data-testid='picker'>{data?.map((a) => a.name).join(",")}</span>;
    });
    expect(await firstRequestTo("/ws-1/agents")).toEqual({ path: "/ws-1/agents", branch: null });
    // …and still offers only what a schedule can run.
    await waitFor(() => expect(screen.getByTestId("picker").textContent).toBe("sales"));
  });

  it("the cluster map's source filter (observability) sends no branch", async () => {
    renderAt(IDE.OBSERVABILITY.CLUSTERS, function SourceFilter() {
      useAgents();
      return null;
    });
    expect(await firstRequestTo("/ws-1/agents")).toEqual({ path: "/ws-1/agents", branch: null });
  });

  it("the header keeps the branch on those pages — it is the IDE's", async () => {
    mocks.Header = ListsAgents;
    renderAt(IDE.COORDINATOR.JOBS, () => null);
    expect(await firstRequestTo("/ws-1/agents")).toEqual({
      path: "/ws-1/agents",
      branch: "feat/x"
    });
  });

  it("an artifact is read without one from anywhere, the IDE included", async () => {
    renderAt(IDE.FILES.ROOT, function Artifact() {
      useEffect(() => {
        void ArtifactService.getArtifact("ws-1", "a-1");
      }, []);
      return null;
    });
    expect(await firstRequestTo("/ws-1/artifacts/a-1")).toEqual({
      path: "/ws-1/artifacts/a-1",
      branch: null
    });
  });

  it("the agent picker and the agent list share one fetch, each with its own shape", async () => {
    // Same key on purpose. Two `queryFn`s under it would have each overwrite
    // the other's list; a `select` gives the picker its filter over one cache.
    renderAt(IDE.COORDINATOR.JOBS, function Both() {
      const all = useAgents();
      const schedulable = useScheduleAgents();
      return (
        <>
          <span data-testid='all'>{all.data?.map((a) => a.name).join(",")}</span>
          <span data-testid='schedulable'>{schedulable.data?.map((a) => a.name).join(",")}</span>
        </>
      );
    });
    await waitFor(() => expect(screen.getByTestId("all").textContent).toBe("sales,nightly"));
    expect(screen.getByTestId("schedulable").textContent).toBe("sales");
    expect(requestsTo("/ws-1/agents")).toHaveLength(1);
  });
});
