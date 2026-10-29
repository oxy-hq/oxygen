// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CustomAppSummary } from "@/types/apps";
import type { InboxItem } from "@/types/notifications";
import CustomerHome from "./index";
import { HOME_CONTAINER, HOME_GAP } from "./layout";

/**
 * The apps-first home: what a customer sees and, as much, what they do not — no
 * setup prompts, no "Not measured" rows, no author's status line, never the word
 * "workspace".
 */

let factoryUser = false;
let gaps: { label: string }[] = [];
let shouldDisableChat = false;
let apps: CustomAppSummary[] = [];
let unread: Record<string, number> = {};
let latest: Record<string, Partial<InboxItem>> = {};

vi.mock("@/hooks/api/notifications", () => ({
  useAppUnreadCounts: () => unread,
  useLatestUnreadByApp: () => latest
}));

vi.mock("@/stores/useCurrentOrg", () => ({
  default: <T,>(selector: (s: unknown) => T) =>
    selector({ org: { id: "org-1", slug: "northwind", name: "Northwind" }, role: "member" })
}));
vi.mock("@/stores/useAskDock", () => ({
  default: <T,>(selector: (s: { open: () => void }) => T) => selector({ open: vi.fn() })
}));
vi.mock("@/hooks/api/users/useCurrentUser", () => ({
  default: () => ({ data: { name: "Priya Silva", email: "priya.silva@northwind.test" } })
}));
vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "ws-1" } })
}));
vi.mock("@/hooks/useFactoryAccess", () => ({ default: () => factoryUser }));
vi.mock("@/hooks/api/customApps/useOrgCustomApps", () => ({
  default: () => ({ apps, isPending: false })
}));
vi.mock("../useWorkspaceReadiness", () => ({
  default: () => ({ status: "ready", gaps, shouldDisableChat })
}));
// Markers for what the page decides to mount; their own behaviour is tested beside them.
vi.mock("../components/ProjectSetupToast", () => ({
  ProjectSetupToast: () => <div data-testid='setup-toast' />
}));
vi.mock("../components/CriticalAlertBanner", () => ({
  CriticalAlertBanner: () => <div data-testid='critical-banner' />
}));
vi.mock("../components/NeedsAttention", () => ({
  NeedsAttention: ({ includeUnmeasured }: { includeUnmeasured: boolean }) => (
    <div data-testid='needs-attention' data-unmeasured={String(includeUnmeasured)} />
  )
}));
let worldModel: { entities: { id: string }[] } | undefined;
vi.mock("@/hooks/api/useWorldModel", () => ({ useWorldModel: () => ({ data: worldModel }) }));
vi.mock("./components/WorldModelCard", () => ({
  worldModelTitle: (org?: string) => `How ${org}'s data fits together`,
  WorldModelCard: ({ to, className }: { to: string; className?: string }) => (
    <a data-testid='world-model-card' href={to} className={className}>
      world model
    </a>
  )
}));
vi.mock("./components/HomeWidgets", () => ({ HomeWidgets: () => null }));
vi.mock("./components/HomeRecentQuestions", () => ({
  HomeRecentQuestions: () => <h2 data-testid='recent-questions'>Your recent questions</h2>
}));
vi.mock("@/components/appAccess/AppAccessDialog", () => ({ AppAccessDialog: () => null }));

const revenue: CustomAppSummary = {
  id: "app-1",
  slug: "revenue",
  name: "Revenue Board",
  org_slug: "northwind",
  url: "/customer-apps/northwind/revenue/",
  published_at: new Date(Date.now() - 3 * 24 * 3600 * 1000).toISOString(),
  description: "Weekly revenue",
  status: "Example app · seeded by oxy seed · queries oxymart",
  visibility: "org"
};

const renderHome = () =>
  render(
    <MemoryRouter>
      <CustomerHome />
    </MemoryRouter>
  );

beforeEach(() => {
  factoryUser = false;
  gaps = [{ label: "Add an LLM key" }];
  shouldDisableChat = false;
  apps = [revenue];
  unread = {};
  latest = {};
  worldModel = { entities: [{ id: "store" }] };
});

const appsOf = (n: number): CustomAppSummary[] =>
  Array.from({ length: n }, (_, i) => ({ ...revenue, id: `app-${i}`, slug: `app-${i}` }));
afterEach(cleanup);

describe("the home's app notifications", () => {
  it("badges an app with its unread count and shows its newest one", () => {
    unread = { "app-1": 2 };
    latest = { "app-1": { title: "Walk-in cooler is warm" } };
    renderHome();
    expect(screen.getByTestId("home-app-card-unread-revenue")).toHaveTextContent("2");
    expect(screen.getByTestId("home-app-card-banner-revenue")).toHaveTextContent(
      "Walk-in cooler is warm"
    );
    expect(screen.getByTestId("launcher-app-card-revenue")).toHaveAccessibleName(
      "Revenue Board, 2 unread"
    );
  });

  it("shows the badge alone when the store did not answer for the text", () => {
    unread = { "app-1": 1 };
    latest = { "app-1": { title: null, body: null } };
    renderHome();
    expect(screen.getByTestId("home-app-card-unread-revenue")).toHaveTextContent("1");
    expect(screen.queryByTestId("home-app-card-banner-revenue")).toBeNull();
  });

  it("shows neither while nothing is unread, even with a stale newest", () => {
    latest = { "app-1": { title: "Old news" } };
    renderHome();
    expect(screen.queryByTestId("home-app-card-unread-revenue")).toBeNull();
    expect(screen.queryByTestId("home-app-card-banner-revenue")).toBeNull();
  });
});

describe("the apps-first home", () => {
  it("greets the person by first name under the org's name", () => {
    renderHome();
    expect(screen.getByTestId("home-org-name")).toHaveTextContent("Northwind");
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(/, Priya$/);
  });

  it("offers the ask box when chat is usable, and not when it is not", () => {
    renderHome();
    expect(screen.getByTestId("home-ask-box")).toBeInTheDocument();
    cleanup();
    shouldDisableChat = true;
    renderHome();
    expect(screen.queryByTestId("home-ask-box")).toBeNull();
  });

  it("shows each app with a freshness line and none of the author's status text", () => {
    renderHome();
    expect(screen.getByTestId("launcher-app-card-revenue")).toBeInTheDocument();
    expect(screen.getByTestId("launcher-app-card-freshness-revenue")).toHaveTextContent(
      "Updated 3 days ago"
    );
    expect(screen.queryByText(/seeded by oxy seed/)).toBeNull();
  });

  it("says the first apps are on their way when there are none yet", () => {
    apps = [];
    renderHome();
    expect(screen.getByTestId("home-first-apps")).toHaveTextContent(
      "Your Oxy team is building Northwind's apps"
    );
    expect(screen.queryByTestId("launcher-app-grid")).toBeNull();
  });

  it("links the world model card to the read-only page", () => {
    renderHome();
    expect(screen.getByTestId("world-model-card")).toHaveAttribute(
      "href",
      "/northwind/workspaces/ws-1/world-model"
    );
    expect(screen.getByText("Your recent questions")).toBeInTheDocument();
  });

  it("keeps setup prompts and unmeasured rows from a customer, and real alerts in", () => {
    renderHome();
    expect(screen.queryByTestId("setup-toast")).toBeNull();
    expect(screen.getByTestId("needs-attention")).toHaveAttribute("data-unmeasured", "false");
    expect(screen.getByTestId("critical-banner")).toBeInTheDocument();
  });

  it("keeps them for a Factory user", () => {
    factoryUser = true;
    renderHome();
    expect(screen.getByTestId("setup-toast")).toBeInTheDocument();
    expect(screen.getByTestId("needs-attention")).toHaveAttribute("data-unmeasured", "true");
  });

  it("never says workspace, with apps or without", () => {
    renderHome();
    expect(document.body.textContent).not.toMatch(/workspace/i);
    cleanup();
    apps = [];
    renderHome();
    expect(document.body.textContent).not.toMatch(/workspace/i);
  });
});

/**
 * The alignment rule: every block sits in the same container, the grids share one
 * gap, and the lower row is cut on the app row's columns, so the line between the
 * two lower cards falls on an app card's edge.
 */
describe("the home's column system", () => {
  const classesOf = (el: Element | null) => (el?.getAttribute("class") ?? "").split(/\s+/);
  const hasAll = (el: Element | null, classes: string) =>
    classes.split(" ").every((c) => classesOf(el).includes(c));

  it("puts the greeting, the ask box, the apps and the lower row in one container", () => {
    renderHome();
    const header = screen.getByTestId("home-ask-box").closest("header");
    const body = screen.getByTestId("home-body");
    expect(hasAll(header, HOME_CONTAINER)).toBe(true);
    expect(hasAll(body, HOME_CONTAINER)).toBe(true);
    // Nothing between the container and a section narrows it.
    expect(screen.getByTestId("home-ask-box").parentElement).toBe(header);
    expect(screen.getByTestId("launcher-app-grid").parentElement?.parentElement).toBe(body);
    expect(screen.getByTestId("home-lower-row").parentElement).toBe(body);
    expect(hasAll(screen.getByTestId("home-ask-box"), "max-w-3xl")).toBe(false);
  });

  it("gives the app grid and the lower row the same gap", () => {
    renderHome();
    expect(hasAll(screen.getByTestId("launcher-app-grid"), HOME_GAP)).toBe(true);
    expect(hasAll(screen.getByTestId("home-lower-row"), HOME_GAP)).toBe(true);
  });

  it("shows one app as one wide card on the thirds", () => {
    apps = appsOf(1);
    renderHome();
    expect(screen.getByTestId("launcher-app-grid")).toHaveAttribute("data-layout", "one");
    expect(hasAll(screen.getByTestId("home-app-card-app-0"), "sm:grid-cols-3")).toBe(true);
    expect(hasAll(screen.getByTestId("home-lower-row"), "lg:grid-cols-3")).toBe(true);
    expect(hasAll(screen.getByTestId("world-model-card"), "lg:col-span-2")).toBe(true);
  });

  it("splits the lower row in half under two apps", () => {
    apps = appsOf(2);
    renderHome();
    expect(hasAll(screen.getByTestId("launcher-app-grid"), "sm:grid-cols-2")).toBe(true);
    expect(hasAll(screen.getByTestId("home-lower-row"), "lg:grid-cols-2")).toBe(true);
    expect(hasAll(screen.getByTestId("world-model-card"), "lg:col-span-2")).toBe(false);
  });

  it("puts three or more apps on the thirds, with the world model card on two of them", () => {
    apps = appsOf(5);
    renderHome();
    expect(hasAll(screen.getByTestId("launcher-app-grid"), "lg:grid-cols-3")).toBe(true);
    expect(hasAll(screen.getByTestId("home-lower-row"), "lg:grid-cols-3")).toBe(true);
    expect(hasAll(screen.getByTestId("world-model-card"), "lg:col-span-2")).toBe(true);
  });

  it.each([8, 9])(
    "puts %i apps on quarters with compact cards, the lower row on the same edges",
    (n) => {
      apps = appsOf(n);
      renderHome();
      const grid = screen.getByTestId("launcher-app-grid");
      expect(grid).toHaveAttribute("data-layout", "dense");
      expect(hasAll(grid, "grid-cols-2 md:grid-cols-3 xl:grid-cols-4")).toBe(true);
      expect(screen.getAllByTestId(/^home-app-card-app-\d+$/)).toHaveLength(n);
      for (const card of screen.getAllByTestId(/^home-app-card-app-\d+$/)) {
        expect(card).toHaveAttribute("data-size", "compact");
      }
      // Two quarters each from 1280 px; on the thirds below that, like the apps.
      const lower = screen.getByTestId("home-lower-row");
      expect(hasAll(lower, "lg:grid-cols-3 xl:grid-cols-2")).toBe(true);
      expect(hasAll(screen.getByTestId("world-model-card"), "lg:col-span-2 xl:col-span-1")).toBe(
        true
      );
    }
  );

  it("keeps five apps on the thirds and moves to quarters from six", () => {
    apps = appsOf(5);
    renderHome();
    expect(screen.getByTestId("launcher-app-grid")).toHaveAttribute("data-layout", "many");
    cleanup();
    apps = appsOf(6);
    renderHome();
    expect(screen.getByTestId("launcher-app-grid")).toHaveAttribute("data-layout", "dense");
  });

  it("draws an app without art as its mark on the art's ground, at the same size", () => {
    apps = appsOf(8).map((a, i) => (i === 5 ? a : { ...a, art_url: `/art-${i}.svg` }));
    renderHome();
    expect(screen.queryByTestId("home-app-card-placeholder-app-4")).toBeNull();
    const placeholder = screen.getByTestId("home-app-card-placeholder-app-5");
    expect(hasAll(placeholder, "aspect-2/1 bg-primary/10")).toBe(true);
    expect(screen.getByTestId("launcher-app-card-app-5")).toHaveTextContent(revenue.name);
  });

  it("gives the recent questions the whole row when there is no world model", () => {
    worldModel = { entities: [] };
    renderHome();
    expect(screen.queryByTestId("world-model-card")).toBeNull();
    expect(hasAll(screen.getByTestId("home-lower-row"), "grid-cols-1")).toBe(true);
    expect(classesOf(screen.getByTestId("home-lower-row")).some((c) => c.startsWith("lg:"))).toBe(
      false
    );
  });
});
