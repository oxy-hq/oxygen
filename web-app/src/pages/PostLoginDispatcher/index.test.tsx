// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import PostLoginDispatcher from ".";

const orgs = vi.hoisted(() => ({ data: [] as { id: string; slug: string }[] }));
const workspaces = vi.hoisted(() => ({ data: [] as { id: string; status: string }[] }));

vi.mock("@/hooks/api/organizations", () => ({
  useOrgs: () => ({ data: orgs.data, isPending: false, isError: false })
}));
const user = vi.hoisted(() => ({
  data: {
    partner_memberships: [] as { partner_id: string }[],
    is_app_admin: false,
    is_owner: false
  }
}));
vi.mock("@/hooks/api/users/useCurrentUser", () => ({
  default: () => ({ data: user.data, isPending: false })
}));
vi.mock("@/hooks/api/workspaces/useWorkspaces", () => ({
  useAllWorkspaces: () => ({ data: workspaces.data, isPending: false, isError: false })
}));
vi.mock("@/libs/orgSubdomain", () => ({ getInjectedOrg: () => null }));
// The apps-first flag (`GET /auth/config`). On for the org-home tests, off for main's.
const appsFirst = vi.hoisted(() => ({ on: true }));
vi.mock("@/hooks/useAppsFirstExperience", () => ({ default: () => appsFirst.on }));
const appCounts = vi.hoisted(() => ({ data: {} as Record<string, number> }));
vi.mock("@/hooks/api/customApps/useWorkspacesCustomApps", () => ({
  useWorkspacesCustomApps: (ids: readonly string[]) => ({
    appsByWorkspace: new Map(
      ids.map((id) => [id, Array.from({ length: appCounts.data[id] ?? 0 })])
    ),
    isPending: false
  })
}));

function Landed() {
  const location = useLocation();
  return <p data-testid='landed'>{`${location.pathname}${location.search}`}</p>;
}

const land = (url: string) => {
  render(
    <MemoryRouter initialEntries={[url]}>
      <Routes>
        <Route index element={<PostLoginDispatcher />} />
        <Route path='*' element={<Landed />} />
      </Routes>
    </MemoryRouter>
  );
  return screen.getByTestId("landed").textContent ?? "";
};

describe("PostLoginDispatcher", () => {
  afterEach(cleanup);

  beforeEach(() => {
    localStorage.clear();
    orgs.data = [{ id: "org-1", slug: "acme" }];
    workspaces.data = [{ id: "ws-1", status: "ready" }];
    appCounts.data = {};
    user.data = { partner_memberships: [], is_app_admin: false, is_owner: false };
    appsFirst.on = true;
  });

  it("carries the token emails' ?settings= link into the page it lands on", () => {
    const landed = land("/?settings=account.tokens");
    expect(landed).toBe("/acme/home?settings=account.tokens");
  });

  it("lands a customer on the org home", () => {
    expect(land("/")).toBe("/acme/home");
  });

  it.each([
    ["an app admin", { is_app_admin: true }],
    ["a platform owner", { is_owner: true }],
    ["a partner's staff", { partner_memberships: [{ partner_id: "p-1" }] }]
  ])("keeps %s landing in a workspace", (_, who) => {
    user.data = { ...user.data, ...who };
    expect(land("/")).toBe("/acme/workspaces/ws-1");
  });

  it("carries it to the org root when there is no workspace yet", () => {
    workspaces.data = [];
    const landed = land("/?settings=organization.api_access");
    expect(landed).toContain("acme");
    expect(landed.endsWith("?settings=organization.api_access")).toBe(true);
  });

  it("carries the page a push notification opens, so the inbox opens there", () => {
    expect(land("/?notifications=open&utm=push")).toBe("/acme/home?notifications=open");
  });

  it("forwards nothing else", () => {
    expect(land("/?utm=mail")).not.toContain("?");
  });

  it("lands a staff member's first visit on the workspace with the most apps", () => {
    user.data = { ...user.data, is_app_admin: true };
    workspaces.data = [
      { id: "ws-empty", status: "ready" },
      { id: "ws-apps", status: "ready" }
    ];
    appCounts.data = { "ws-apps": 2 };
    expect(land("/")).toBe("/acme/workspaces/ws-apps");
  });
});

describe("PostLoginDispatcher with the apps-first experience off", () => {
  afterEach(cleanup);

  beforeEach(() => {
    localStorage.clear();
    orgs.data = [{ id: "org-1", slug: "acme" }];
    workspaces.data = [{ id: "ws-1", status: "ready" }];
    appCounts.data = {};
    user.data = { partner_memberships: [], is_app_admin: false, is_owner: false };
    appsFirst.on = false;
  });

  it("lands a customer in the workspace, as before the org home", () => {
    expect(land("/")).toBe("/acme/workspaces/ws-1");
  });

  it("carries the token emails' ?settings= link into the workspace it picks", () => {
    expect(land("/?settings=account.tokens")).toBe("/acme/workspaces/ws-1?settings=account.tokens");
  });

  it("carries the page a push notification opens into the workspace", () => {
    expect(land("/?notifications=open&utm=push")).toBe("/acme/workspaces/ws-1?notifications=open");
  });
});
