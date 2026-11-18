// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import WorkspaceEntry from "./WorkspaceEntry";

/**
 * The old workspace address stays live for every link that carries it: emails,
 * Slack, push notifications, bookmarks, the server's own deep links. It must
 * remember the workspace it names, then land on the org address of the same
 * page with nothing lost from the link.
 */

let orgPending: boolean;

vi.mock("@/components/useResolveOrgForGuard", () => ({
  useResolveOrgForGuard: () => ({
    org: orgPending ? undefined : { id: "org-1", slug: "acme", name: "Acme" },
    isGlobalOverride: false,
    isPending: orgPending
  })
}));

function Landed() {
  const { pathname, search, hash, state } = useLocation();
  return <div data-testid='landed'>{`${pathname}${search}${hash}|${JSON.stringify(state)}`}</div>;
}

const renderAt = (entry: string | { pathname: string; state: unknown }) =>
  render(
    <MemoryRouter initialEntries={[entry]}>
      <Routes>
        <Route path=':orgSlug/workspaces/:wsId/*' element={<WorkspaceEntry />} />
        <Route path=':orgSlug/*' element={<Landed />} />
      </Routes>
    </MemoryRouter>
  );

const remembered = () => localStorage.getItem("oxy:last_workspace:user-1:org-1");

beforeEach(() => {
  orgPending = false;
  localStorage.clear();
  localStorage.setItem("user", JSON.stringify({ id: "user-1" }));
});
afterEach(cleanup);

describe("WorkspaceEntry", () => {
  it("remembers the workspace and lands on the same page without it", () => {
    renderAt("/acme/workspaces/ws-b/automations/YQ?run=r-1#step-2");
    expect(remembered()).toBe("ws-b");
    expect(screen.getByTestId("landed")).toHaveTextContent("/acme/automations/YQ?run=r-1#step-2");
  });

  it("carries the navigation state across", () => {
    renderAt({ pathname: "/acme/workspaces/ws-b/threads", state: { agentPath: "a.yml" } });
    expect(screen.getByTestId("landed")).toHaveTextContent('/acme/threads|{"agentPath":"a.yml"}');
  });

  it("sends the workspace's root to the org home", () => {
    renderAt("/acme/workspaces/ws-b?settings=organization.integration");
    expect(screen.getByTestId("landed")).toHaveTextContent(
      "/acme/home?settings=organization.integration"
    );
  });

  it("waits for the org before remembering anything", () => {
    orgPending = true;
    renderAt("/acme/workspaces/ws-b/threads");
    expect(screen.queryByTestId("landed")).toBeNull();
    expect(remembered()).toBeNull();
  });
});
