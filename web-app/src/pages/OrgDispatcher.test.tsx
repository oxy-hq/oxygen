// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Organization, OrgRole } from "@/types/organization";
import OrgDispatcher from "./OrgDispatcher";

/**
 * `/:orgSlug` for an org with nothing ready. It used to redirect into a
 * self-serve onboarding wizard; orgs are provisioned for their members now, so
 * it renders "being set up" in place — and must never strand an admin who is
 * mid-way through creating a workspace.
 */

type Workspace = { id: string; status: "ready" | "cloning" | "failed" };
type User = {
  email: string;
  is_owner: boolean;
  is_app_admin: boolean;
  partner_memberships?: { partner_id: string }[];
};

const MEMBER_USER: User = { email: "crew@acme.test", is_owner: false, is_app_admin: false };
const STAFF_USER: User = { ...MEMBER_USER, is_app_admin: true };

let org: Organization;
let workspaces: Workspace[];
let user: User;

vi.mock("@/hooks/api/organizations", () => ({
  useOrgs: () => ({ data: [org], isPending: false })
}));
vi.mock("@/hooks/api/workspaces/useWorkspaces", () => ({
  useAllWorkspaces: () => ({ data: workspaces, isPending: false, isError: false })
}));
// Published-app counts per workspace, for the landing order. Only the length matters.
let appCounts: Record<string, number> = {};
let countsPending = false;
vi.mock("@/hooks/api/customApps/useWorkspacesCustomApps", () => ({
  useWorkspacesCustomApps: (ids: readonly string[]) => ({
    appsByWorkspace: new Map(ids.map((id) => [id, Array.from({ length: appCounts[id] ?? 0 })])),
    isPending: ids.length > 0 && countsPending
  })
}));
vi.mock("@/hooks/api/users/useCurrentUser", () => ({
  default: () => ({ data: user, isPending: false })
}));
// The apps-first flag as `GET /auth/config` reports it; off unless a test turns it on.
let appsFirst = false;
vi.mock("@/contexts/AuthContext", () => ({
  useAuth: () => ({ logout: vi.fn() }),
  useOptionalAuth: () => ({
    isLocalMode: false,
    authConfig: { apps_first_experience: appsFirst }
  })
}));
vi.mock("@/components/OxyLogo", () => ({ default: () => null }));
vi.mock("@/components/settings/SettingsDialog", () => ({ default: () => null }));
vi.mock("@/components/settings/SettingsDialog/useSettingsDeepLink", () => ({
  SETTINGS_PARAM: "settings",
  useSettingsDeepLink: () => {}
}));
vi.mock("@/stores/useSettingsDialog", () => ({
  default: <T,>(selector: (s: { open: () => void }) => T) => selector({ open: vi.fn() })
}));
// The creator itself (GitHub / demo / blank, polling, wizard hand-off) is out
// of scope; a marker is enough to see whether the dispatcher kept it mounted.
vi.mock("@/components/org/OrgSetupPending/components/CreateWorkspacePanel", () => ({
  default: () => <div data-testid='creator' />
}));

function Location() {
  const { pathname, search } = useLocation();
  return (
    <div data-testid='location'>
      {pathname}
      {search}
    </div>
  );
}

const renderAt = (entry = "/acme") =>
  render(
    <MemoryRouter initialEntries={[entry]}>
      <Routes>
        <Route path=':orgSlug' element={<OrgDispatcher />} />
        <Route path='*' element={<Location />} />
      </Routes>
    </MemoryRouter>
  );

const setRole = (role: OrgRole) => {
  org = { ...org, role };
};
// What `GET /orgs` reports for the staff who may provision here.
const allowProvisioning = () => {
  org = { ...org, may_provision_workspace: true };
};

beforeEach(() => {
  org = { id: "org-1", name: "Acme", slug: "acme", role: "member" };
  workspaces = [];
  user = MEMBER_USER;
  appCounts = {};
  countsPending = false;
  appsFirst = false;
  localStorage.clear();
});
afterEach(cleanup);

describe("OrgDispatcher — an org with nothing ready", () => {
  it("shows 'being set up' in place, not onboarding", () => {
    renderAt();
    expect(screen.getByTestId("org-setup-pending-title")).toHaveTextContent("Acme is being set up");
    expect(screen.queryByTestId("location")).toBeNull();
  });

  it("treats workspaces that are all still cloning as nothing ready", () => {
    workspaces = [{ id: "ws-1", status: "cloning" }];
    renderAt();
    expect(screen.getByTestId("org-setup-pending")).toBeInTheDocument();
  });

  it("offers a plain member no way to create a workspace", () => {
    renderAt();
    expect(screen.queryByTestId("org-setup-pending-create-workspace")).toBeNull();
    expect(screen.queryByTestId("org-setup-pending-open-settings")).toBeNull();
    expect(screen.getByTestId("org-setup-pending-log-out")).toBeInTheDocument();
  });

  it.each<OrgRole>(["owner", "admin"])(
    "offers an org %s organization settings, not the setup wizard",
    (role) => {
      // The card says the Oxygen team is setting the org up; a customer Owner
      // being handed "Create workspace" as well would contradict it.
      setRole(role);
      renderAt();
      expect(screen.queryByTestId("org-setup-pending-create-workspace")).toBeNull();
      expect(screen.getByTestId("org-setup-pending-open-settings")).toBeInTheDocument();
    }
  );

  it("does not offer staff who are a plain Member here a button the server refuses", () => {
    // Every creator endpoint takes OrgAdmin, which reads the org role only.
    user = { ...MEMBER_USER, is_app_admin: true };
    renderAt();
    expect(screen.queryByTestId("org-setup-pending-create-workspace")).toBeNull();
    // Org settings follow the org role, not platform standing.
    expect(screen.queryByTestId("org-setup-pending-open-settings")).toBeNull();
  });

  it("offers Create workspace to staff holding an Owner role (e.g. an assume-role session)", () => {
    user = { ...MEMBER_USER, is_app_admin: true };
    setRole("owner");
    allowProvisioning();
    renderAt();
    expect(screen.getByTestId("org-setup-pending-create-workspace")).toBeInTheDocument();
    expect(screen.getByTestId("org-setup-pending-open-settings")).toBeInTheDocument();
  });

  it("keeps the creator mounted when the new workspace turns ready", () => {
    // The created workspace flips the list to ready before the preparing screen
    // has handed off to the setup wizard. Navigating to the workspace here would
    // unmount the creator and skip that hand-off.
    user = { ...MEMBER_USER, is_app_admin: true };
    setRole("owner");
    allowProvisioning();
    const { rerender } = renderAt();
    fireEvent.click(screen.getByTestId("org-setup-pending-create-workspace"));
    expect(screen.getByTestId("creator")).toBeInTheDocument();

    workspaces = [{ id: "ws-new", status: "ready" }];
    rerender(
      <MemoryRouter initialEntries={["/acme"]}>
        <Routes>
          <Route path=':orgSlug' element={<OrgDispatcher />} />
          <Route path='*' element={<Location />} />
        </Routes>
      </MemoryRouter>
    );
    expect(screen.getByTestId("creator")).toBeInTheDocument();
    expect(screen.queryByTestId("location")).toBeNull();
  });
});

describe("OrgDispatcher — an org with nothing ready, apps-first", () => {
  beforeEach(() => {
    appsFirst = true;
  });

  it("tells a customer their Oxy team is setting their apps up, with nothing to create", () => {
    renderAt();
    expect(screen.getByTestId("org-setup-pending")).toHaveTextContent(
      "Your Oxy team is setting up your apps"
    );
    expect(screen.queryByTestId("org-setup-pending-create-workspace")).toBeNull();
    expect(screen.queryByTestId("org-setup-pending-bootstrap")).toBeNull();
  });

  it("gives a customer's own owner the organization settings and nothing to create", () => {
    setRole("owner");
    renderAt();
    expect(screen.getByTestId("org-setup-pending-open-settings")).toBeInTheDocument();
    expect(screen.queryByTestId("org-setup-pending-create-workspace")).toBeNull();
    expect(screen.queryByTestId("org-setup-pending-bootstrap")).toBeNull();
  });

  it("gives staff who may set the org up the oxyc commands, not the web creator", () => {
    user = STAFF_USER;
    setRole("owner");
    allowProvisioning();
    renderAt();
    expect(screen.queryByTestId("org-setup-pending-create-workspace")).toBeNull();
    const script = screen.getByTestId("org-setup-pending-bootstrap-script");
    expect(script).toHaveTextContent("oxyc new acme-oxy --display 'Acme'");
    expect(script).toHaveTextContent("orgs/org-1/onboarding/github");
    expect(screen.getByTestId("org-setup-pending-bootstrap-blank")).toHaveTextContent(
      "oxyc api -X POST orgs/org-1/onboarding/new -f name='Acme'"
    );
  });

  it("shows the commands to nobody the server would refuse", () => {
    user = STAFF_USER;
    setRole("owner");
    org = { ...org, may_provision_workspace: false };
    renderAt();
    expect(screen.queryByTestId("org-setup-pending-bootstrap")).toBeNull();
  });
});

describe("OrgDispatcher — an org with a ready workspace", () => {
  beforeEach(() => {
    appsFirst = true;
  });

  it("sends a customer to the org home", () => {
    workspaces = [{ id: "ws-1", status: "ready" }];
    renderAt();
    expect(screen.getByTestId("location")).toHaveTextContent(/^\/acme\/home$/);
  });

  it("sends an operator straight into the workspace", () => {
    workspaces = [{ id: "ws-1", status: "ready" }];
    user = STAFF_USER;
    renderAt();
    expect(screen.getByTestId("location")).toHaveTextContent(/^\/acme\/workspaces\/ws-1$/);
  });

  it("sends a partner's staff straight into the workspace", () => {
    workspaces = [{ id: "ws-1", status: "ready" }];
    user = { ...MEMBER_USER, partner_memberships: [{ partner_id: "p-1" }] };
    renderAt();
    expect(screen.getByTestId("location")).toHaveTextContent(/^\/acme\/workspaces\/ws-1$/);
  });

  it("a platform owner who is not an app admin is staff too", () => {
    workspaces = [{ id: "ws-1", status: "ready" }];
    user = { ...MEMBER_USER, is_owner: true };
    renderAt();
    expect(screen.getByTestId("location")).toHaveTextContent(/^\/acme\/workspaces\/ws-1$/);
  });

  it("carries a settings deep link into the page it lands on, and nothing else", () => {
    // `/kiosk` links a browser that is not a tablet to `/<org>?settings=
    // organization.crew`; the workspace layout is what opens the dialog, so the
    // section has to survive the hop.
    workspaces = [{ id: "ws-1", status: "ready" }];
    renderAt("/acme?settings=organization.crew&slack_installed=ok");
    expect(screen.getByTestId("location")).toHaveTextContent(
      /^\/acme\/home\?settings=organization\.crew$/
    );
  });

  it("carries the inbox deep link a push notification opens", () => {
    workspaces = [{ id: "ws-1", status: "ready" }];
    renderAt("/acme?notifications=open&slack_installed=ok");
    expect(screen.getByTestId("location")).toHaveTextContent(/^\/acme\/home\?notifications=open$/);
  });
});

describe("OrgDispatcher — an org with several workspaces", () => {
  beforeEach(() => {
    appsFirst = true;
    // The prod repro: the API lists an app-less workspace first.
    workspaces = [
      { id: "ws-empty", status: "ready" },
      { id: "ws-apps", status: "ready" }
    ];
    appCounts = { "ws-apps": 3 };
  });

  it("lands staff on the workspace with the most apps", () => {
    user = STAFF_USER;
    renderAt();
    expect(screen.getByTestId("location")).toHaveTextContent(/^\/acme\/workspaces\/ws-apps$/);
  });

  it("waits for the counts instead of landing on the first in the list", () => {
    user = STAFF_USER;
    countsPending = true;
    renderAt();
    expect(screen.queryByTestId("location")).toBeNull();
  });

  it("keeps the workspace this staff member opened last", () => {
    user = STAFF_USER;
    localStorage.setItem("user", JSON.stringify({ id: "user-1" }));
    localStorage.setItem("oxy:last_workspace:user-1:org-1", "ws-empty");
    renderAt();
    expect(screen.getByTestId("location")).toHaveTextContent(/^\/acme\/workspaces\/ws-empty$/);
  });

  it("sends a customer to the org home, which picks the workspace itself", () => {
    renderAt();
    expect(screen.getByTestId("location")).toHaveTextContent(/^\/acme\/home$/);
  });
});

describe("OrgDispatcher — the apps-first experience off", () => {
  it("sends a customer into the workspace, as before the org home", () => {
    workspaces = [{ id: "ws-1", status: "ready" }];
    renderAt();
    expect(screen.getByTestId("location")).toHaveTextContent(/^\/acme\/workspaces\/ws-1$/);
  });

  it("carries a settings deep link into the workspace", () => {
    workspaces = [{ id: "ws-1", status: "ready" }];
    renderAt("/acme?settings=organization.crew&slack_installed=ok");
    expect(screen.getByTestId("location")).toHaveTextContent(
      /^\/acme\/workspaces\/ws-1\?settings=organization\.crew$/
    );
  });
});
