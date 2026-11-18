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
  ])("lands %s on the org home too: the workspace is gone from the address", (_, who) => {
    user.data = { ...user.data, ...who };
    expect(land("/")).toBe("/acme/home");
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

  it("remembers the workspace with the most apps on a first visit, for the home to load from", () => {
    localStorage.setItem("user", JSON.stringify({ id: "user-1" }));
    user.data = { ...user.data, is_app_admin: true };
    workspaces.data = [
      { id: "ws-empty", status: "ready" },
      { id: "ws-apps", status: "ready" }
    ];
    appCounts.data = { "ws-apps": 2 };
    expect(land("/")).toBe("/acme/home");
    expect(localStorage.getItem("oxy:last_workspace:user-1:org-1")).toBe("ws-apps");
  });
});
