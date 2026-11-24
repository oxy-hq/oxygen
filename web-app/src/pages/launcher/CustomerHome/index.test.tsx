// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CustomAppSummary } from "@/types/apps";
import CustomerHome from ".";

/**
 * The home, direction A: a greeting and the apps as preview cards,
 * nothing else on the page. The sort orders them by what this person opened
 * last, or by name; owners and admins get the access gear; a phone shows the
 * notifications in place of the apps from the bell beside the greeting.
 */

const app = (
  id: string,
  name: string,
  extra: Partial<CustomAppSummary> = {}
): CustomAppSummary => ({
  id,
  slug: id,
  name,
  org_slug: "northwind",
  url: `/customer-apps/northwind/${id}/`,
  published_at: "2026-11-05T00:00:00Z",
  visibility: "org",
  ...extra
});

let apps: CustomAppSummary[] = [];
let role = "member";
let desk = true;
let factoryUser = false;

vi.mock("@/stores/useCurrentOrg", () => ({
  default: <T,>(selector: (s: unknown) => T) =>
    selector({ org: { id: "org-1", slug: "northwind", name: "Northwind" }, role })
}));
vi.mock("@/stores/useAskDock", () => ({
  default: <T,>(selector: (s: { open: () => void }) => T) => selector({ open: vi.fn() })
}));
vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "ws-1" } })
}));
vi.mock("@/hooks/useFactoryAccess", () => ({ default: () => factoryUser }));
vi.mock("@/hooks/api/customApps/useOrgCustomApps", () => ({
  default: () => ({ apps, isPending: false })
}));
vi.mock("@/hooks/api/users/useCurrentUser", () => ({
  default: () => ({ data: { name: "Sam Lee" } })
}));
vi.mock("@/hooks/api/agents/useWorkspaceTimezone", () => ({ default: () => "UTC" }));
vi.mock("@/hooks/useMediaQuery", () => ({ useMediaQuery: () => desk }));
vi.mock("@/hooks/api/notifications", () => ({
  useUnreadNotificationCount: () => ({ data: 2 })
}));
vi.mock("../useWorkspaceReadiness", () => ({
  default: () => ({ status: "ready", gaps: [], shouldDisableChat: false })
}));
vi.mock("../components/ProjectSetupToast", () => ({
  ProjectSetupToast: () => <div data-testid='setup-toast' />
}));
vi.mock("../components/CriticalAlertBanner", () => ({
  CriticalAlertBanner: () => <div data-testid='critical-banner' />
}));
vi.mock("@/components/appAccess/AppAccessDialog", () => ({
  AppAccessDialog: ({ open, appName }: { open: boolean; appName: string }) =>
    open ? <div data-testid='access-dialog'>{appName}</div> : null
}));
vi.mock("@/components/Notifications/NoticeList", () => ({
  NoticeList: () => <div data-testid='notice-list' />
}));

const renderHome = () =>
  render(
    <MemoryRouter>
      <CustomerHome />
    </MemoryRouter>
  );
const cardOrder = () =>
  within(screen.getByTestId("launcher-app-grid"))
    .getAllByTestId(/^home-app-card-/)
    .map((el) => el.dataset.testid ?? "")
    .filter((id) => !id.startsWith("home-app-card-access-"))
    .map((id) => id.replace("home-app-card-", ""));

// jsdom lays nothing out: the cut lines read glyph boxes from a Range, which it lacks.
if (!Range.prototype.getBoundingClientRect) {
  Range.prototype.getBoundingClientRect = () => new DOMRect();
}

beforeEach(() => {
  window.localStorage.clear();
  apps = [app("zeta", "Zeta Ops"), app("alpha", "alpha books"), app("mid", "Middle")];
});
afterEach(() => {
  cleanup();
  role = "member";
  desk = true;
  factoryUser = false;
});

describe("the home", () => {
  it("is the greeting and the apps, and nothing else", () => {
    renderHome();
    expect(screen.getByTestId("home-greeting").textContent).toMatch(
      /^Good (morning|afternoon|evening), Sam$/
    );
    expect(screen.getByRole("heading", { name: "Your apps" })).toBeTruthy();
    expect(cardOrder()).toEqual(["zeta", "alpha", "mid"]);
    // No ask box, Today, recent questions or data map.
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(screen.queryByText(/Today/)).toBeNull();
    expect(screen.queryByText(/recent questions/i)).toBeNull();
    expect(screen.queryByTestId("world-model-card")).toBeNull();
    expect(screen.queryByTestId("home-phone-bell")).toBeNull();
    // An app that is down still says so.
    expect(screen.getByTestId("critical-banner")).toBeTruthy();
  });

  it("draws each app as a card that opens it, with its art, its name and when it was published", () => {
    apps = [app("zeta", "Zeta Ops", { art_url: "/art/zeta.webp" }), app("mid", "Middle")];
    renderHome();
    const zeta = screen.getByTestId("home-app-card-zeta");
    expect(zeta.querySelector("img")?.getAttribute("src")).toBe("/art/zeta.webp");
    expect(within(zeta).getByTestId("launcher-app-card-zeta").getAttribute("href")).toBe(
      "/customer-apps/northwind/zeta/"
    );
    // One line, ending in "…" when too long, so a row of cards keeps one height.
    expect(within(zeta).getByTestId("launcher-app-card-zeta").firstElementChild?.className).toMatch(
      /\btruncate\b/
    );
    expect(within(zeta).getByTestId("launcher-app-card-freshness-zeta").textContent).toMatch(
      /^Updated /
    );
    // No art: the letter tile, never an empty frame.
    const mid = screen.getByTestId("home-app-card-mid");
    expect(mid.querySelector("img")).toBeNull();
    expect(within(mid).getAllByText("M").length).toBeGreaterThan(0);
  });

  it("sorts by what was opened last, then by name", () => {
    window.localStorage.setItem("oxy.home.recent.northwind", JSON.stringify(["mid", "alpha"]));
    renderHome();
    expect(cardOrder()).toEqual(["mid", "alpha", "zeta"]);
    fireEvent.click(screen.getByTestId("home-apps-sort"));
    fireEvent.click(screen.getByTestId("home-apps-sort-alpha"));
    expect(cardOrder()).toEqual(["alpha", "mid", "zeta"]);
    expect(screen.getByTestId("home-apps-sort").getAttribute("aria-label")).toBe(
      "Sort apps: Alphabetical"
    );
  });

  it("gives owners and admins the access gear, and nobody else", () => {
    renderHome();
    expect(screen.queryByTestId("home-app-card-access-zeta")).toBeNull();
    cleanup();
    role = "admin";
    apps = [app("zeta", "Zeta Ops", { visibility: "members" })];
    renderHome();
    const gear = screen.getByTestId("home-app-card-access-zeta");
    expect(gear.getAttribute("aria-label")).toBe("Manage access to Zeta Ops, restricted");
    fireEvent.click(gear);
    expect(screen.getByTestId("access-dialog").textContent).toBe("Zeta Ops");
  });

  it("on a phone, shows the notifications in place of the apps from the bell beside the greeting", () => {
    desk = false;
    renderHome();
    const bell = screen.getByTestId("home-phone-bell");
    expect(bell.getAttribute("aria-label")).toBe("Notifications, 2 unread");
    fireEvent.click(bell);
    expect(screen.getByTestId("home-greeting").textContent).toBe("Notifications");
    expect(screen.getByTestId("notice-list")).toBeTruthy();
    expect(screen.queryByTestId("launcher-app-grid")).toBeNull();
    fireEvent.click(screen.getByTestId("home-phone-bell"));
    expect(screen.getByTestId("launcher-app-grid")).toBeTruthy();
  });

  it("shows the setup toast to a Factory user only", () => {
    renderHome();
    expect(screen.queryByTestId("setup-toast")).toBeNull();
    cleanup();
    factoryUser = true;
    renderHome();
    expect(screen.getByTestId("setup-toast")).toBeTruthy();
  });
});
