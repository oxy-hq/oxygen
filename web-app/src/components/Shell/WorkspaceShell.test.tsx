// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { CustomAppSummary } from "@/types/apps";
import { WorkspaceShell } from "./WorkspaceShell";

/**
 * The rail's app marks are the org's, not one workspace's: the same list the org
 * home's grid shows, so the rail does not change as a person moves between Home,
 * Chat and a workspace page. HQ goes to the org home.
 */

const card = (id: string): CustomAppSummary => ({
  id,
  slug: id,
  name: id,
  org_slug: "acme",
  url: `/customer-apps/acme/${id}/`,
  published_at: "2026-07-30T00:00:00Z",
  visibility: "org"
});

// One app in each of two workspaces; the hook has already merged them.
const orgApps = [card("revenue"), card("forecast")];
const askedWith = vi.fn();
const askedWithOptions = vi.fn();

vi.mock("@/hooks/api/customApps/useOrgCustomApps", () => ({
  default: (workspaceId: string, options?: { acrossOrg?: boolean }) => {
    askedWith(workspaceId);
    askedWithOptions(options);
    return { apps: orgApps, isPending: false };
  }
}));
let operator = false;
// What the staff member's grant lets them do; an App Operator's holds no
// `operate_platform`.
let capabilities = ["operate_platform"];
vi.mock("@/hooks/api/users/useCurrentUser", () => ({
  default: () => ({
    data: {
      is_owner: false,
      is_app_admin: operator,
      platform_capabilities: operator ? capabilities : []
    }
  })
}));
let appsFirst = false;
// Unread per app, as the bell's poll reports it; what the hook was enabled with.
let unreadByApp: Record<string, number> = {};
const unreadEnabled = vi.fn();
vi.mock("@/hooks/api/notifications", () => ({
  useAppUnreadCounts: (enabled: boolean) => {
    unreadEnabled(enabled);
    return enabled ? unreadByApp : {};
  }
}));
vi.mock("@/hooks/useAppsFirstExperience", () => ({ default: () => appsFirst }));
// The dialog itself is tested on its own; here only whether the rail opens it.
vi.mock("@/components/admin/OpenFactoryDialog", () => ({
  OpenFactoryDialog: ({
    open,
    initialScope
  }: {
    open: boolean;
    initialScope?: { orgName: string } | null;
  }) =>
    open ? <div data-testid='open-factory-dialog'>{initialScope?.orgName ?? "all"}</div> : null
}));
vi.mock("@/stores/useCurrentOrg", () => ({
  default: <T,>(selector: (s: { org: { id: string; slug: string; name: string } }) => T) =>
    selector({ org: { id: "org-acme", slug: "acme", name: "Acme" } })
}));
// A live assume-role session, for the staff Factory button.
let actingAs: string | null = null;
vi.mock("@/hooks/api/adminAssume/useActingSession", () => ({
  useActingSession: () => ({
    session: actingAs ? { org_id: actingAs } : undefined,
    isActing: !!actingAs
  })
}));
vi.mock("@/stores/useCurrentWorkspace", () => ({
  default: () => ({ workspace: { id: "ws-1" } })
}));

// Chrome with queries and stores of its own, not under test.
vi.mock("@/components/Ask/AskDock", () => ({ AskDock: () => null }));
vi.mock("@/components/PreviewBar", () => ({ PreviewBar: () => null }));
vi.mock("@/components/WorkspaceStatus", () => ({ default: () => null }));
vi.mock("./TopBar", () => ({ TopBar: () => null }));
vi.mock("./RailUserMenu", () => ({ RailUserMenu: () => null }));
vi.mock("./RailWorkspaceSwitch", () => ({
  RailWorkspaceSwitch: () => <div data-testid='rail-workspace-switch' />
}));
vi.mock("./RailWorkspaceTile", () => ({ RailWorkspaceTile: () => null }));

function Location() {
  return <div data-testid='location'>{useLocation().pathname}</div>;
}

const renderAt = (entry: string) =>
  render(
    <MemoryRouter initialEntries={[entry]}>
      <Routes>
        <Route
          path='*'
          element={
            <WorkspaceShell>
              <Location />
            </WorkspaceShell>
          }
        />
      </Routes>
    </MemoryRouter>
  );

afterEach(() => {
  cleanup();
  appsFirst = false;
  operator = false;
  capabilities = ["operate_platform"];
  actingAs = null;
  unreadByApp = {};
});

describe("the rail's unread badges", () => {
  it("puts each app's unread count on its mark with the apps-first experience", () => {
    appsFirst = true;
    unreadByApp = { revenue: 3 };
    renderAt("/acme/home");
    expect(screen.getByTestId("rail-app-revenue-unread")).toHaveTextContent("3");
    expect(screen.getByTestId("rail-app-revenue")).toHaveAccessibleName("revenue, 3 unread");
    expect(screen.queryByTestId("rail-app-forecast-unread")).toBeNull();
  });

  it("draws none, and asks for none, with the flag off", () => {
    unreadByApp = { revenue: 3 };
    renderAt("/acme/home");
    expect(unreadEnabled).toHaveBeenLastCalledWith(false);
    expect(screen.queryByTestId("rail-app-revenue-unread")).toBeNull();
    expect(screen.getByTestId("rail-app-revenue")).toHaveAccessibleName("revenue");
  });
});

describe("WorkspaceShell rail", () => {
  it("shows a mark for every app in the org, whichever workspace holds it", () => {
    renderAt("/acme/home");
    expect(screen.getByTestId("rail-app-revenue")).toHaveAttribute(
      "href",
      "/customer-apps/acme/revenue/"
    );
    expect(screen.getByTestId("rail-app-forecast")).toBeInTheDocument();
  });

  it("asks for the org's apps from the workspace the page loads from", () => {
    renderAt("/acme/home");
    expect(askedWith).toHaveBeenLastCalledWith("ws-1");
  });

  it("shows the same marks on a workspace page", () => {
    renderAt("/acme/workspaces/ws-1/threads");
    expect(screen.getByTestId("rail-app-revenue")).toBeInTheDocument();
    expect(screen.getByTestId("rail-app-forecast")).toBeInTheDocument();
  });

  it("takes Home to the org home with the apps-first experience", () => {
    appsFirst = true;
    renderAt("/acme/workspaces/ws-1/threads");
    fireEvent.click(screen.getByTestId("rail-hq"));
    expect(screen.getByTestId("location")).toHaveTextContent(/^\/acme\/home$/);
  });

  it("takes HQ to the workspace's home without it, as before the org home", () => {
    renderAt("/acme/workspaces/ws-1/threads");
    fireEvent.click(screen.getByTestId("rail-hq"));
    expect(screen.getByTestId("location")).toHaveTextContent(/^\/acme\/workspaces\/ws-1\/home$/);
  });

  it("is never asked for the org's other workspaces without it", () => {
    renderAt("/acme/home");
    expect(askedWithOptions).toHaveBeenLastCalledWith({ acrossOrg: false });
  });

  it("marks HQ active on the org home", () => {
    renderAt("/acme/home");
    expect(screen.getByTestId("rail-hq").className).toContain("bg-muted");
    expect(screen.getByTestId("rail-chat").className).not.toContain("bg-muted");
  });
});

describe("WorkspaceShell rail, apps-first", () => {
  it("is as it was with the flag off: HQ, and the set switcher", () => {
    renderAt("/acme/home");
    expect(screen.getByTestId("rail-hq")).toHaveAccessibleName(/HQ/);
    expect(screen.getByTestId("rail-workspace-switch")).toBeInTheDocument();
  });

  it("says Home and drops the set switcher with the flag on", () => {
    appsFirst = true;
    renderAt("/acme/home");
    expect(screen.getByTestId("rail-hq")).toHaveAccessibleName(/Home/);
    expect(screen.queryByTestId("rail-workspace-switch")).toBeNull();
  });

  it("keeps Home lit on the read-only world model", () => {
    appsFirst = true;
    renderAt("/acme/workspaces/ws-1/world-model");
    expect(screen.getByTestId("rail-hq").className).toContain("bg-muted");
  });
});

describe("the staff Factory button in the rail", () => {
  it("opens the Open Factory flow on this org for Oxy staff, in place of the plain entry", () => {
    appsFirst = true;
    operator = true;
    renderAt("/acme/home");
    expect(screen.queryByTestId("rail-core")).toBeNull();
    fireEvent.click(screen.getByTestId("rail-open-factory"));
    expect(screen.getByTestId("open-factory-dialog")).toHaveTextContent("Acme");
  });

  it("shows the Oxygen Factory mark, not a terminal icon", () => {
    appsFirst = true;
    operator = true;
    renderAt("/acme/home");
    const mark = screen.getByTestId("rail-open-factory").querySelector("img");
    expect(mark?.getAttribute("src")).toMatch(/oxygen-factory-mark-(light|dark)\.svg$/);
  });

  it("goes straight into the Factory when staff already act as this org", () => {
    appsFirst = true;
    operator = true;
    actingAs = "org-acme";
    renderAt("/acme/workspaces/ws-1/threads");
    fireEvent.click(screen.getByTestId("rail-open-factory"));
    expect(screen.queryByTestId("open-factory-dialog")).toBeNull();
    expect(screen.getByTestId("location")).toHaveTextContent(/^\/acme\/workspaces\/ws-1\/ide$/);
  });

  /// `Ring::Factory` admits staff by `operate_platform`. An App Operator is staff
  /// (`is_app_admin`) without it, so the button would only ever lead to a 403.
  it("is not shown to an App Operator, who keeps the plain entry", () => {
    appsFirst = true;
    operator = true;
    capabilities = ["manage_apps", "develop_apps"];
    renderAt("/acme/home");
    expect(screen.queryByTestId("rail-open-factory")).toBeNull();
    expect(screen.getByTestId("rail-core")).toBeInTheDocument();
  });

  it("is never shown to a customer, an org owner or admin included", () => {
    appsFirst = true;
    renderAt("/acme/home");
    expect(screen.queryByTestId("rail-open-factory")).toBeNull();
    expect(screen.getByTestId("rail-core")).toBeInTheDocument();
  });

  it("is not there with the flag off: the rail is main's", () => {
    operator = true;
    renderAt("/acme/home");
    expect(screen.queryByTestId("rail-open-factory")).toBeNull();
    expect(screen.getByTestId("rail-core")).toBeInTheDocument();
  });
});
