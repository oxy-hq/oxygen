// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import type { Workspace } from "@/types/workspace";
import { WorkspaceShell } from "./WorkspaceShell";

// The shell's own children fetch and subscribe to things this test is not about.
vi.mock("@/hooks/api/adminAssume/useActingSession", () => ({
  useActingSession: () => ({ session: undefined, isActing: false })
}));
vi.mock("@/components/Ask/AskDock", () => ({ AskDock: () => null }));
vi.mock("@/components/PreviewBar", () => ({ PreviewBar: () => null }));
vi.mock("@/components/WorkspaceStatus", () => ({ default: () => null }));
vi.mock("./RailUserMenu", () => ({ RailUserMenu: () => null }));
vi.mock("./RailWorkspaceSwitch", () => ({ RailWorkspaceSwitch: () => null }));
vi.mock("./RailWorkspaceTile", () => ({ RailWorkspaceTile: () => null }));
vi.mock("./TopBar", () => ({ TopBar: () => null }));
vi.mock("@/hooks/api/notifications", () => ({ useAppUnreadCounts: () => ({}) }));
vi.mock("@/hooks/api/customApps/useCustomApps", () => ({
  useCustomApps: () => ({ data: [] })
}));
vi.mock("@/hooks/api/customApps/useOrgCustomApps", () => ({
  default: () => ({ apps: [], isPending: false })
}));
vi.mock("@/hooks/api/users/useCurrentUser", () => ({
  default: () => ({ data: { is_owner: false, is_app_admin: false, partner_memberships: [] } })
}));

const workspace = (factory_access?: boolean): Workspace =>
  ({
    id: "ws-1",
    name: "Acme",
    ...(factory_access === undefined ? {} : { factory_access })
  }) as Workspace;

function renderShell() {
  render(
    <MemoryRouter initialEntries={["/acme/workspaces/ws-1/home"]}>
      <WorkspaceShell>
        <div />
      </WorkspaceShell>
    </MemoryRouter>
  );
}

beforeEach(() => {
  useCurrentOrg.setState({ org: { id: "org-1", slug: "acme" } as never, role: "member" });
});

afterEach(() => {
  cleanup();
  useCurrentWorkspace.setState({ workspace: null });
});

describe("the rail's Oxygen Factory entry", () => {
  /// The server says who may use the Factory; the rail follows it. A customer
  /// member, admin and owner are sent `factory_access: false`.
  it("is hidden when the server says the caller may not use the Factory", () => {
    useCurrentWorkspace.setState({ workspace: workspace(false) });
    renderShell();
    expect(screen.getByTestId("rail-hq")).toBeInTheDocument();
    expect(screen.queryByTestId("rail-core")).not.toBeInTheDocument();
  });

  it("is shown to someone the server lets in", () => {
    useCurrentWorkspace.setState({ workspace: workspace(true) });
    renderShell();
    expect(screen.getByTestId("rail-core")).toBeInTheDocument();
  });

  /// While enforcement is off the server sends `true` to everyone, and a server
  /// that predates the field sends nothing: either way the rail is what it was.
  it("is shown when the response does not carry the field", () => {
    useCurrentWorkspace.setState({ workspace: workspace(undefined) });
    renderShell();
    expect(screen.getByTestId("rail-core")).toBeInTheDocument();
  });
});
