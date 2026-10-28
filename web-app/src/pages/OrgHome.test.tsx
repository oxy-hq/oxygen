// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import OrgHome from "./OrgHome";

/**
 * `/<org>/home` loads the shell from one workspace — the landing rule, as it
 * always was — while the page itself lists the whole org. These pin which
 * workspace it hands the layout, and that an org with nothing ready goes back to
 * the org root (which shows "being set up") instead of rendering an empty shell.
 */

type Workspace = { id: string; status: "ready" | "cloning" | "failed" };

let workspaces: Workspace[];
let appCounts: Record<string, number>;

// The apps-first flag: the org home exists only with it on.
const appsFirst = vi.hoisted(() => ({ on: true }));
vi.mock("@/hooks/useAppsFirstExperience", () => ({ default: () => appsFirst.on }));
vi.mock("@/hooks/api/organizations", () => ({
  useOrgs: () => ({ data: [{ id: "org-1", slug: "acme", name: "Acme" }], isPending: false })
}));
vi.mock("@/hooks/api/workspaces/useWorkspaces", () => ({
  useAllWorkspaces: () => ({ data: workspaces, isPending: false, isError: false })
}));
vi.mock("@/hooks/api/customApps/useWorkspacesCustomApps", () => ({
  useWorkspacesCustomApps: (ids: readonly string[]) => ({
    appsByWorkspace: new Map(ids.map((id) => [id, Array.from({ length: appCounts[id] ?? 0 })])),
    isPending: false
  })
}));

function Layout({ workspaceId, orgHome }: { workspaceId?: string; orgHome?: boolean }) {
  return <div data-testid='layout'>{`${workspaceId}:${orgHome ? "home" : "workspace"}`}</div>;
}

function Location() {
  const { pathname } = useLocation();
  return <div data-testid='location'>{pathname}</div>;
}

const renderHome = () =>
  render(
    <MemoryRouter initialEntries={["/acme/home"]}>
      <Routes>
        <Route path=':orgSlug/home' element={<OrgHome Layout={Layout} />} />
        <Route path='*' element={<Location />} />
      </Routes>
    </MemoryRouter>
  );

beforeEach(() => {
  workspaces = [];
  appCounts = {};
  localStorage.clear();
  appsFirst.on = true;
});
afterEach(cleanup);

describe("OrgHome", () => {
  it("renders the org home from the workspace with the most apps", () => {
    workspaces = [
      { id: "ws-empty", status: "ready" },
      { id: "ws-apps", status: "ready" }
    ];
    appCounts = { "ws-apps": 3 };
    renderHome();
    expect(screen.getByTestId("layout")).toHaveTextContent("ws-apps:home");
  });

  it("renders it from the workspace this person opened last", () => {
    workspaces = [
      { id: "ws-empty", status: "ready" },
      { id: "ws-apps", status: "ready" }
    ];
    appCounts = { "ws-apps": 3 };
    localStorage.setItem("user", JSON.stringify({ id: "user-1" }));
    localStorage.setItem("oxy:last_workspace:user-1:org-1", "ws-empty");
    renderHome();
    expect(screen.getByTestId("layout")).toHaveTextContent("ws-empty:home");
  });

  it("never loads from a workspace that is still cloning", () => {
    workspaces = [
      { id: "ws-cloning", status: "cloning" },
      { id: "ws-ready", status: "ready" }
    ];
    renderHome();
    expect(screen.getByTestId("layout")).toHaveTextContent("ws-ready:home");
  });

  it("goes back to the org root when nothing is ready, rather than render an empty shell", () => {
    workspaces = [{ id: "ws-cloning", status: "cloning" }];
    renderHome();
    expect(screen.queryByTestId("layout")).toBeNull();
    expect(screen.getByTestId("location")).toHaveTextContent(/^\/acme$/);
  });
});

describe("OrgHome with the apps-first experience off", () => {
  it("goes to the org root, which lands in a workspace as before", () => {
    appsFirst.on = false;
    workspaces = [{ id: "ws-apps", status: "ready" }];
    renderHome();
    expect(screen.queryByTestId("layout")).toBeNull();
    expect(screen.getByTestId("location")).toHaveTextContent(/^\/acme$/);
  });
});
