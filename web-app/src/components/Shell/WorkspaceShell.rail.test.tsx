// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CustomAppSummary } from "@/types/apps";
import { WorkspaceShell } from "./WorkspaceShell";

/**
 * The shell draws the reference design's rail: it
 * opens from a toggle at its head (not on hover), narrows the page while open,
 * remembers that in this browser, lists the apps A to Z under a title, draws
 * each app as a mark in the rail's grey rather than its coloured icon, and draws
 * no unread badges (the bell's dot does that job). It has no Chat entry.
 */

const card = (id: string, name: string): CustomAppSummary => ({
  id,
  slug: id,
  name,
  org_slug: "acme",
  url: `/customer-apps/acme/${id}/`,
  published_at: "2026-07-30T00:00:00Z",
  visibility: "org"
});

vi.mock("@/hooks/api/customApps/useOrgCustomApps", () => ({
  default: () => ({
    apps: [
      { ...card("zeta", "Zeta Ops"), icon_url: "/customer-apps/acme/zeta/icon.svg" },
      card("alpha", "alpha books"),
      card("mid", "Middle")
    ],
    isPending: false
  })
}));
vi.mock("@/hooks/api/users/useCurrentUser", () => ({
  default: () => ({ data: { is_owner: false, is_app_admin: false, platform_capabilities: [] } })
}));
vi.mock("@/components/admin/OpenFactoryDialog", () => ({ OpenFactoryDialog: () => null }));
vi.mock("@/hooks/api/organizations", () => ({ useOrgs: () => ({ data: [] }) }));
vi.mock("@/stores/useCurrentOrg", () => ({
  default: <T,>(selector: (s: { org: { id: string; slug: string; name: string } }) => T) =>
    selector({ org: { id: "org-acme", slug: "acme", name: "Acme" } })
}));
vi.mock("@/hooks/api/adminAssume/useActingSession", () => ({
  useActingSession: () => ({ session: undefined, isActing: false })
}));
vi.mock("@/stores/useCurrentWorkspace", () => ({
  default: () => ({ workspace: { id: "ws-1", name: "Acme Analytics" } })
}));
vi.mock("@/components/Ask/AskDock", () => ({ AskDock: () => null }));
vi.mock("@/components/PreviewBar", () => ({ PreviewBar: () => null }));
vi.mock("./TopBar", () => ({
  TopBar: ({ bellOnPage }: { bellOnPage?: boolean }) => (
    <div data-testid='topbar' data-bell-on-page={bellOnPage ? "yes" : "no"} />
  )
}));
vi.mock("./NotificationsDock", () => ({
  NotificationsDock: () => <div data-testid='notifications-dock' />
}));
vi.mock("./RailUserMenu", () => ({
  RailUserMenu: ({ open }: { open?: boolean }) => (
    <span data-testid='account' data-open={open ? "yes" : "no"} />
  )
}));
vi.mock("./RailWorkspaceTile", () => ({ RailWorkspaceTile: () => <span data-testid='logo' /> }));

const renderAt = (entry: string) =>
  render(
    <MemoryRouter initialEntries={[entry]}>
      <WorkspaceShell>
        <div />
      </WorkspaceShell>
    </MemoryRouter>
  );

beforeEach(() => window.localStorage.clear());
afterEach(cleanup);

describe("WorkspaceShell's rail", () => {
  it("starts with the rail closed, names hidden but still read out", () => {
    renderAt("/acme/home");
    const rail = screen.getByTestId("shell-rail");
    expect(rail.dataset.open).toBeUndefined();
    expect(rail.className).toContain("w-12");
    expect(screen.getByTestId("shell-rail-toggle").getAttribute("aria-expanded")).toBe("false");
    // The name is in the row, hidden from sight only.
    expect(within(screen.getByTestId("rail-hq")).getByText("Home").className).toContain("sr-only");
    expect(screen.getAllByTestId("account")[0].dataset.open).toBe("no");
    expect(screen.queryByText("Apps")).toBeNull();
  });

  it("opens from its toggle to 224px, shows the org's name and the apps' title, and remembers it", () => {
    renderAt("/acme/home");
    fireEvent.click(screen.getByTestId("shell-rail-toggle"));
    const rail = screen.getByTestId("shell-rail");
    expect(rail.dataset.open).toBe("true");
    expect(rail.className).toContain("w-56");
    expect(screen.getByTestId("shell-rail-name").textContent).toBe("Acme");
    expect(screen.getByText("Apps")).toBeTruthy();
    expect(within(screen.getByTestId("rail-hq")).getByText("Home").className).not.toContain(
      "sr-only"
    );
    expect(window.localStorage.getItem("oxy.shell-rail.expanded")).toBe("true");
    // Open, the account menu is a row with the person's name, like the entries above it.
    expect(screen.getAllByTestId("account")[0].dataset.open).toBe("yes");
    cleanup();
    renderAt("/acme/home");
    expect(screen.getByTestId("shell-rail").dataset.open).toBe("true");
    fireEvent.click(screen.getByTestId("shell-rail-toggle"));
    expect(screen.getByTestId("shell-rail").dataset.open).toBeUndefined();
    expect(window.localStorage.getItem("oxy.shell-rail.expanded")).toBe("false");
  });

  it("lists the apps A to Z and draws no unread badge", () => {
    renderAt("/acme/home");
    const apps = screen.getAllByTestId(/^rail-app-/).map((el) => el.dataset.testid);
    expect(apps).toEqual(["rail-app-alpha", "rail-app-mid", "rail-app-zeta"]);
    expect(screen.queryByTestId("rail-app-zeta-unread")).toBeNull();
    expect(screen.getByTestId("rail-app-zeta").getAttribute("href")).toBe(
      "/customer-apps/acme/zeta/"
    );
  });

  it("draws an app's icon as a mark in the row's colour, never the coloured picture", () => {
    renderAt("/acme/home");
    const zeta = screen.getByTestId("rail-app-zeta");
    expect(zeta.querySelector("img")).toBeNull();
    expect(zeta.querySelector("[data-glyph]")).not.toBeNull();
  });

  it("marks Home as the current page, and has no Chat entry", () => {
    renderAt("/acme/home");
    expect(screen.getByTestId("rail-hq").getAttribute("aria-current")).toBe("page");
    expect(screen.queryByTestId("rail-chat")).toBeNull();
  });

  it("mounts the notifications panel, and the home draws its own bell on a phone", () => {
    renderAt("/acme/home");
    expect(screen.getByTestId("notifications-dock")).toBeTruthy();
    expect(screen.getByTestId("topbar").dataset.bellOnPage).toBe("yes");
    cleanup();
    renderAt("/acme/workspaces/ws-1/threads");
    expect(screen.getByTestId("topbar").dataset.bellOnPage).toBe("no");
  });
});
