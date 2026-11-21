// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import ROUTES from "@/libs/utils/routes";
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

vi.mock("@/hooks/api/customApps/useOrgCustomApps", () => ({
  default: (workspaceId: string) => {
    askedWith(workspaceId);
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
// Unread per app, as the bell's poll reports it; what the hook was enabled with.
let unreadByApp: Record<string, number> = {};
const unreadEnabled = vi.fn();
vi.mock("@/hooks/api/notifications", () => ({
  useAppUnreadCounts: (enabled: boolean) => {
    unreadEnabled(enabled);
    return enabled ? unreadByApp : {};
  }
}));
// The dialog itself is tested on its own; here only whether the rail opens it.
vi.mock("@/components/admin/OpenFactoryDialog", () => ({
  OpenFactoryDialog: ({
    open,
    initialTarget,
    initialScope
  }: {
    open: boolean;
    initialTarget?: { workspaceName: string } | null;
    initialScope?: { orgName: string } | null;
  }) =>
    open ? (
      <div data-testid='open-factory-dialog'>
        {initialScope?.orgName ?? "all"} / {initialTarget?.workspaceName ?? "none"}
      </div>
    ) : null
}));
// The orgs the person is a real member of.
let memberOf: string[] = [];
vi.mock("@/hooks/api/organizations", () => ({
  useOrgs: () => ({ data: memberOf.map((id) => ({ id })) })
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
  default: () => ({ workspace: { id: "ws-1", name: "Acme Analytics" } })
}));

// Chrome with queries and stores of its own, not under test.
vi.mock("@/components/Ask/AskDock", () => ({ AskDock: () => null }));
vi.mock("@/components/PreviewBar", () => ({ PreviewBar: () => null }));
vi.mock("./TopBar", () => ({ TopBar: () => null }));
vi.mock("./RailUserMenu", () => ({ RailUserMenu: () => null }));
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
  operator = false;
  capabilities = ["operate_platform"];
  actingAs = null;
  memberOf = [];
  unreadByApp = {};
});

describe("the rail's unread badges", () => {
  it("puts each app's unread count on its mark", () => {
    unreadByApp = { revenue: 3 };
    renderAt("/acme/home");
    expect(screen.getByTestId("rail-app-revenue-unread")).toHaveTextContent("3");
    expect(screen.getByTestId("rail-app-revenue")).toHaveAccessibleName("revenue, 3 unread");
    expect(screen.queryByTestId("rail-app-forecast-unread")).toBeNull();
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
    renderAt("/acme/threads");
    expect(screen.getByTestId("rail-app-revenue")).toBeInTheDocument();
    expect(screen.getByTestId("rail-app-forecast")).toBeInTheDocument();
  });

  it("takes Home to the org home", () => {
    renderAt("/acme/threads");
    fireEvent.click(screen.getByTestId("rail-hq"));
    expect(screen.getByTestId("location")).toHaveTextContent(/^\/acme\/home$/);
  });

  it("marks Home active on the org home", () => {
    renderAt("/acme/home");
    expect(screen.getByTestId("rail-hq").className).toContain("bg-muted");
    expect(screen.getByTestId("rail-chat").className).not.toContain("bg-muted");
  });
});

describe("WorkspaceShell rail, Home", () => {
  it("says Home and has no set switcher", () => {
    renderAt("/acme/home");
    expect(screen.getByTestId("rail-hq")).toHaveAccessibleName(/Home/);
    expect(screen.queryByTestId("rail-workspace-switch")).toBeNull();
  });

  it("keeps Home lit on the read-only world model", () => {
    renderAt("/acme/world-model");
    expect(screen.getByTestId("rail-hq").className).toContain("bg-muted");
  });
});

describe("the staff Factory button in the rail", () => {
  it("opens the Open Factory flow at this workspace's reason when a session is needed", () => {
    operator = true;
    renderAt("/acme/home");
    expect(screen.queryByTestId("rail-core")).toBeNull();
    fireEvent.click(screen.getByTestId("rail-open-factory"));
    expect(screen.getByTestId("open-factory-dialog")).toHaveTextContent("Acme / Acme Analytics");
  });

  // The dialog used to open, pick the org's only workspace, and close again on the
  // way into the Factory: a flash that read as a glitch.
  it("goes straight into the Factory, with no dialog, in an org the staff member belongs to", () => {
    operator = true;
    memberOf = ["org-acme"];
    renderAt("/acme/threads");
    expect(screen.getByTestId("rail-open-factory")).toHaveAccessibleName(/Oxygen Nexus/);
    fireEvent.click(screen.getByTestId("rail-open-factory"));
    expect(screen.queryByTestId("open-factory-dialog")).toBeNull();
    expect(screen.getByTestId("location")).toHaveTextContent(/^\/acme\/ide$/);
  });

  it("shows the Oxygen Nexus mark, not a terminal icon", () => {
    operator = true;
    renderAt("/acme/home");
    const mark = screen.getByTestId("rail-open-factory").querySelector("img");
    expect(mark?.getAttribute("src")).toMatch(/oxygen-factory-mark-(light|dark)\.svg$/);
  });

  it("goes straight into the Factory when staff already act as this org", () => {
    operator = true;
    actingAs = "org-acme";
    renderAt("/acme/threads");
    fireEvent.click(screen.getByTestId("rail-open-factory"));
    expect(screen.queryByTestId("open-factory-dialog")).toBeNull();
    expect(screen.getByTestId("location")).toHaveTextContent(/^\/acme\/ide$/);
  });

  /// `Ring::Factory` admits staff by `operate_platform`. An App Operator is staff
  /// (`is_app_admin`) without it, so the button would only ever lead to a 403.
  it("is not shown to an App Operator, who keeps the plain entry", () => {
    operator = true;
    capabilities = ["manage_apps", "develop_apps"];
    renderAt("/acme/home");
    expect(screen.queryByTestId("rail-open-factory")).toBeNull();
    expect(screen.getByTestId("rail-core")).toBeInTheDocument();
  });

  it("is never shown to a customer, an org owner or admin included", () => {
    renderAt("/acme/home");
    expect(screen.queryByTestId("rail-open-factory")).toBeNull();
    expect(screen.getByTestId("rail-core")).toBeInTheDocument();
  });
});

describe("WorkspaceShell on the setup wizard", () => {
  // The wizard owns the screen: no rail, no top bar, no Ask dock.
  it("hides the rail at the wizard's address", () => {
    renderAt(ROUTES.ORG("acme").WORKSPACE().ONBOARDING);
    expect(screen.queryByTestId("rail-hq")).toBeNull();
  });

  it("keeps the rail on another page that merely has a `setup` segment", () => {
    renderAt("/acme/threads/setup");
    expect(screen.getByTestId("rail-hq")).toBeInTheDocument();
  });
});
