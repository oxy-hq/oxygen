// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Organization, OrgRole } from "@/types/organization";
import OrgDispatcher from "./OrgDispatcher";

/**
 * `/:orgSlug` for an org with nothing ready. It used to redirect into a
 * self-serve onboarding wizard; orgs are provisioned for their members now, so
 * it renders "being set up" in place.
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
vi.mock("@/contexts/AuthContext", () => ({
  useAuth: () => ({ logout: vi.fn() }),
  useOptionalAuth: () => ({ isLocalMode: false, authConfig: {} })
}));
vi.mock("@/components/OxyLogo", () => ({ default: () => null }));
// Settings is a route now; the dispatcher only forwards its parameter, so the real
// `settingsRoute` module (constants and pure parsing) is what it should be reading.
vi.mock("@/components/settings/LegacySettingsParamRedirect", () => ({
  LegacySettingsParamRedirect: () => null
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
    user = { ...MEMBER_USER, is_app_admin: true };
    renderAt();
    expect(screen.queryByTestId("org-setup-pending-create-workspace")).toBeNull();
    // Org settings follow the org role, not platform standing.
    expect(screen.queryByTestId("org-setup-pending-open-settings")).toBeNull();
  });
});

describe("OrgDispatcher — an org with nothing ready, what each person is offered", () => {
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
    // A workspace exists only once a repository backs it: no blank start.
    expect(screen.queryByTestId("org-setup-pending-bootstrap-blank")).toBeNull();
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
  it("sends a customer to the org home", () => {
    workspaces = [{ id: "ws-1", status: "ready" }];
    renderAt();
    expect(screen.getByTestId("location")).toHaveTextContent(/^\/acme\/home$/);
  });

  // Staff once went into the workspace; with the workspace gone from the address
  // that is the org home too.
  it.each([
    ["an operator", STAFF_USER],
    ["a partner's staff", { ...MEMBER_USER, partner_memberships: [{ partner_id: "p-1" }] }],
    ["a platform owner", { ...MEMBER_USER, is_owner: true }]
  ])("sends %s to the org home as well", (_who, who) => {
    workspaces = [{ id: "ws-1", status: "ready" }];
    user = who;
    renderAt();
    expect(screen.getByTestId("location")).toHaveTextContent(/^\/acme\/home$/);
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
    // The prod repro: the API lists an app-less workspace first.
    workspaces = [
      { id: "ws-empty", status: "ready" },
      { id: "ws-apps", status: "ready" }
    ];
    appCounts = { "ws-apps": 3 };
  });

  const remembered = () => localStorage.getItem("oxy:last_workspace:user-1:org-1");

  it("remembers the workspace with the most apps for the home to load from", () => {
    user = STAFF_USER;
    localStorage.setItem("user", JSON.stringify({ id: "user-1" }));
    renderAt();
    expect(screen.getByTestId("location")).toHaveTextContent(/^\/acme\/home$/);
    expect(remembered()).toBe("ws-apps");
  });

  it("waits for the counts instead of landing on the first in the list", () => {
    user = STAFF_USER;
    countsPending = true;
    renderAt();
    expect(screen.queryByTestId("location")).toBeNull();
  });

  it("keeps the workspace this person opened last", () => {
    user = STAFF_USER;
    localStorage.setItem("user", JSON.stringify({ id: "user-1" }));
    localStorage.setItem("oxy:last_workspace:user-1:org-1", "ws-empty");
    renderAt();
    expect(screen.getByTestId("location")).toHaveTextContent(/^\/acme\/home$/);
    expect(remembered()).toBe("ws-empty");
  });

  it("sends a customer to the org home, which picks the workspace itself", () => {
    renderAt();
    expect(screen.getByTestId("location")).toHaveTextContent(/^\/acme\/home$/);
  });
});
