// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation, useNavigate } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import OrgWorkspace from "./OrgWorkspace";

/**
 * `/<org>/<page>` loads the shell from the org's workspace, picked by the
 * landing rule while some orgs still have several. These pin which workspace it
 * hands the layout, that the pick holds while the person moves between the org's
 * pages, that a different org picks again, and that an org with nothing ready
 * goes back to the org root (which shows "being set up") instead of rendering an
 * empty shell.
 */

type Workspace = { id: string; status: "ready" | "cloning" | "failed" };

const ORGS = {
  acme: { id: "org-1", slug: "acme", name: "Acme" },
  beta: { id: "org-2", slug: "beta", name: "Beta" }
} as const;

let workspacesByOrg: Record<string, Workspace[]>;
let appCounts: Record<string, number>;

vi.mock("@/components/useResolveOrgForGuard", () => ({
  useResolveOrgForGuard: (slug: keyof typeof ORGS) => ({
    org: ORGS[slug],
    isGlobalOverride: false,
    isPending: false
  })
}));
vi.mock("@/hooks/api/workspaces/useWorkspaces", () => ({
  useAllWorkspaces: (orgId?: string) => ({
    data: orgId ? workspacesByOrg[orgId] : undefined,
    isPending: false,
    isError: false
  })
}));
vi.mock("@/hooks/api/customApps/useWorkspacesCustomApps", () => ({
  useWorkspacesCustomApps: (ids: readonly string[]) => ({
    appsByWorkspace: new Map(ids.map((id) => [id, Array.from({ length: appCounts[id] ?? 0 })])),
    isPending: false
  })
}));

function Layout({ workspaceId }: { workspaceId?: string }) {
  const navigate = useNavigate();
  const { pathname } = useLocation();
  return (
    <div>
      <div data-testid='layout'>{workspaceId}</div>
      <div data-testid='page'>{pathname}</div>
      <button type='button' onClick={() => navigate("/acme/threads")}>
        to threads
      </button>
      <button type='button' onClick={() => navigate("/beta/home")}>
        to beta
      </button>
    </div>
  );
}

function Location() {
  const { pathname } = useLocation();
  return <div data-testid='location'>{pathname}</div>;
}

const renderAt = (path: string) =>
  render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route path=':orgSlug' element={<Location />} />
        <Route path=':orgSlug/*' element={<OrgWorkspace Layout={Layout} />} />
      </Routes>
    </MemoryRouter>
  );

const remember = (orgId: string, workspaceId: string) =>
  localStorage.setItem(`oxy:last_workspace:user-1:${orgId}`, workspaceId);

beforeEach(() => {
  workspacesByOrg = {};
  appCounts = {};
  localStorage.clear();
  localStorage.setItem("user", JSON.stringify({ id: "user-1" }));
});
afterEach(cleanup);

describe("OrgWorkspace", () => {
  it("renders the org's pages from its workspace, with no workspace in the address", () => {
    workspacesByOrg = { "org-1": [{ id: "ws-only", status: "ready" }] };
    renderAt("/acme/threads/t-1");
    expect(screen.getByTestId("layout")).toHaveTextContent("ws-only");
    expect(screen.getByTestId("page")).toHaveTextContent("/acme/threads/t-1");
  });

  it("loads from the workspace with the most apps when an org still has several", () => {
    workspacesByOrg = {
      "org-1": [
        { id: "ws-empty", status: "ready" },
        { id: "ws-apps", status: "ready" }
      ]
    };
    appCounts = { "ws-apps": 3 };
    renderAt("/acme/home");
    expect(screen.getByTestId("layout")).toHaveTextContent("ws-apps");
  });

  it("loads from the workspace this person opened last", () => {
    workspacesByOrg = {
      "org-1": [
        { id: "ws-empty", status: "ready" },
        { id: "ws-apps", status: "ready" }
      ]
    };
    appCounts = { "ws-apps": 3 };
    remember("org-1", "ws-empty");
    renderAt("/acme/home");
    expect(screen.getByTestId("layout")).toHaveTextContent("ws-empty");
  });

  it("keeps its workspace while the person moves between the org's pages", () => {
    workspacesByOrg = {
      "org-1": [
        { id: "ws-a", status: "ready" },
        { id: "ws-b", status: "ready" }
      ]
    };
    remember("org-1", "ws-a");
    renderAt("/acme/home");
    expect(screen.getByTestId("layout")).toHaveTextContent("ws-a");

    // Another tab opens the org's other workspace, which becomes the remembered one.
    remember("org-1", "ws-b");
    fireEvent.click(screen.getByText("to threads"));

    expect(screen.getByTestId("page")).toHaveTextContent("/acme/threads");
    expect(screen.getByTestId("layout")).toHaveTextContent("ws-a");
  });

  it("picks again in a different org", () => {
    workspacesByOrg = {
      "org-1": [{ id: "ws-acme", status: "ready" }],
      "org-2": [{ id: "ws-beta", status: "ready" }]
    };
    renderAt("/acme/home");
    expect(screen.getByTestId("layout")).toHaveTextContent("ws-acme");

    fireEvent.click(screen.getByText("to beta"));

    expect(screen.getByTestId("page")).toHaveTextContent("/beta/home");
    expect(screen.getByTestId("layout")).toHaveTextContent("ws-beta");
  });

  it("never loads from a workspace that is still cloning", () => {
    workspacesByOrg = {
      "org-1": [
        { id: "ws-cloning", status: "cloning" },
        { id: "ws-ready", status: "ready" }
      ]
    };
    renderAt("/acme/home");
    expect(screen.getByTestId("layout")).toHaveTextContent("ws-ready");
  });

  it("goes back to the org root when nothing is ready, rather than render an empty shell", () => {
    workspacesByOrg = { "org-1": [{ id: "ws-cloning", status: "cloning" }] };
    renderAt("/acme/home");
    expect(screen.queryByTestId("layout")).toBeNull();
    expect(screen.getByTestId("location")).toHaveTextContent(/^\/acme$/);
  });
});
