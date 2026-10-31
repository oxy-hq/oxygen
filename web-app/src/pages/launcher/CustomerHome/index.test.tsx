// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CustomAppSummary } from "@/types/apps";
import type { HomeSection } from "@/types/homeWidgets";
import type { InboxItem } from "@/types/notifications";
import CustomerHome from "./index";
import { HOME_CONTAINER } from "./layout";

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
vi.mock("./components/HomeWidgets", () => ({
  HomeWidgets: ({ variant }: { variant: string }) => (
    <div data-testid='home-widgets-stub' data-variant={variant} />
  ),
  TODAY_ANCHOR: "home-today",
  needsCount: (w: { view?: { level?: string } }[] = []) =>
    w.filter((c) => c.view?.level === "attention" || c.view?.level === "critical").length
}));
const W: HomeSection = { kind: "widgets", title: null, items: [] };
let sections: HomeSection[] = [W, { kind: "apps" }, { kind: "insights" }];
type Widgets = { data?: { view?: { level?: string } }[]; isLoading: boolean; isError: boolean };
let widgets: Widgets = { data: [], isLoading: false, isError: false };
const someWidgets = (...levels: string[]): Widgets => ({
  data: levels.map((level) => ({ view: { level } })),
  isLoading: false,
  isError: false
});
vi.mock("@/hooks/api/homeWidgets/useHomeWidgets", () => ({
  useHomeSections: () => sections,
  useHomeWidgets: () => widgets
}));
// Wide (the rail) unless a test sets a phone.
let wide = true;
vi.mock("@/hooks/useMediaQuery", () => ({ useMediaQuery: () => wide }));
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
  sections = [W, { kind: "apps" }, { kind: "insights" }];
  widgets = { data: [], isLoading: false, isError: false };
  wide = true;
  unread = {};
  latest = {};
  worldModel = { entities: [{ id: "store" }] };
});

const appsOf = (n: number): CustomAppSummary[] =>
  Array.from({ length: n }, (_, i) => ({ ...revenue, id: `app-${i}`, slug: `app-${i}` }));
afterEach(cleanup);

describe("the home's frame", () => {
  it("puts Today in a rail beside the apps on a wide screen, each scrolling on its own", () => {
    widgets = someWidgets("critical", "ok");
    renderHome();
    expect(screen.getByTestId("customer-home")).toHaveAttribute("data-today", "rail");
    const rail = screen.getByTestId("home-today-rail");
    expect(rail.tagName).toBe("ASIDE");
    expect(rail).toHaveClass("overflow-y-auto");
    expect(rail.querySelector("[data-testid=home-widgets-stub]")).toHaveAttribute(
      "data-variant",
      "rail"
    );
    expect(screen.getByTestId("launcher-app-grid")).toHaveAttribute("data-beside-rail", "true");
  });

  it("puts the apps first and Today stacked under them on a narrow screen", () => {
    widgets = someWidgets("critical");
    wide = false;
    renderHome();
    expect(screen.queryByTestId("home-today-rail")).toBeNull();
    const grid = screen.getByTestId("launcher-app-grid");
    const today = screen.getByTestId("home-widgets-stub");
    expect(today).toHaveAttribute("data-variant", "stack");
    expect(screen.getByTestId("customer-home")).toHaveAttribute("data-today", "stack");
    // The apps come before Today on the page, as they do on the screen.
    expect(grid.compareDocumentPosition(today) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("gives the apps the whole width when the org has no widgets", () => {
    renderHome();
    expect(screen.getByTestId("customer-home")).toHaveAttribute("data-today", "none");
    expect(screen.queryByTestId("home-widgets-stub")).toBeNull();
    expect(screen.getByTestId("launcher-app-grid")).toHaveAttribute("data-beside-rail", "false");
  });

  it("holds the rail open while the widgets load, so the apps do not reflow", () => {
    widgets = { data: undefined, isLoading: true, isError: false };
    renderHome();
    expect(screen.getByTestId("customer-home")).toHaveAttribute("data-today", "rail");
  });

  it("always draws the apps; insights only while the layout keeps them", () => {
    sections = [W];
    renderHome();
    expect(screen.getByTestId("launcher-app-grid")).toBeInTheDocument();
    expect(screen.queryByTestId("home-lower-row")).toBeNull();
  });
});

describe("the line under the greeting", () => {
  it("counts the widgets that need someone and links to Today", () => {
    widgets = someWidgets("critical", "attention", "ok");
    renderHome();
    const status = screen.getByTestId("home-status");
    expect(status).toHaveTextContent("2 things need you today");
    expect(status).toHaveAttribute("href", "#home-today");
  });

  it("says one thing in the singular, and all clear when nothing needs anyone", () => {
    widgets = someWidgets("attention", "ok");
    renderHome();
    expect(screen.getByTestId("home-status")).toHaveTextContent("1 thing needs you today");
    cleanup();
    widgets = someWidgets("ok", "info");
    renderHome();
    expect(screen.getByTestId("home-status")).toHaveTextContent("All clear today");
  });

  it("says nothing while there are no widgets", () => {
    renderHome();
    expect(screen.queryByTestId("home-status")).toBeNull();
  });
});

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
  it("greets the person by first name; the org's name is the top bar's, not repeated", () => {
    renderHome();
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(/, Priya$/);
    expect(screen.queryByText("Northwind")).toBeNull();
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

describe("the home's container", () => {
  it("puts the greeting, the apps and what is under them in the one container", () => {
    renderHome();
    const body = screen.getByTestId("home-body");
    expect(body.className.split(/\s+/)).toEqual(expect.arrayContaining(HOME_CONTAINER.split(" ")));
    expect(body.contains(screen.getByTestId("home-ask-box"))).toBe(true);
    expect(body.contains(screen.getByTestId("launcher-app-grid"))).toBe(true);
    expect(body.contains(screen.getByTestId("home-lower-row"))).toBe(true);
  });

  it("puts every app on the one grid, however many there are", () => {
    apps = appsOf(30);
    renderHome();
    expect(screen.getByTestId("launcher-app-grid").children).toHaveLength(30);
  });

  it("draws an app with no icon as its tinted two-letter monogram", () => {
    apps = [{ ...revenue, icon_url: undefined }];
    renderHome();
    const mark = screen.getByTestId("launcher-app-card-mark-revenue");
    expect(mark).toHaveTextContent("RB");
    expect(mark.className).toMatch(/bg-chart-\d/);
  });
});
