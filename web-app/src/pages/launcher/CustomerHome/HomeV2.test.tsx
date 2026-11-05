// @vitest-environment jsdom

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CustomAppSummary } from "@/types/apps";
import type { HomeSection } from "@/types/homeWidgets";
import type { InboxItem } from "@/types/notifications";
import CustomerHome from "./HomeV2";
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
vi.mock("@/hooks/api/agents/useWorkspaceTimezone", () => ({
  default: () => "America/Los_Angeles"
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
let worldModelLoading = false;
vi.mock("@/hooks/api/useWorldModel", () => ({
  useWorldModel: () => ({ data: worldModel, isLoading: worldModelLoading })
}));
vi.mock("./components/WorldModelCard", () => ({
  worldModelTitle: (org?: string) => `How ${org}'s data fits together`,
  WorldModelCard: ({ to, className }: { to: string; className?: string }) => (
    <a data-testid='world-model-card' href={to} className={className}>
      world model
    </a>
  )
}));
vi.mock("./components/HomeWidgets", () => ({
  HomeWidgets: ({ variant, enabled }: { variant: string; enabled?: boolean }) => (
    <div data-testid='home-widgets-stub' data-variant={variant} data-enabled={String(enabled)} />
  ),
  TODAY_ANCHOR: "home-today",
  needsCount: (w: { view?: { level?: string } }[] = []) =>
    w.filter((c) => c.view?.level === "attention" || c.view?.level === "critical").length
}));
const W: HomeSection = { kind: "widgets", title: null, items: [] };
let sections: HomeSection[] = [W, { kind: "apps" }, { kind: "insights" }];
type Widgets = {
  data?: {
    key?: string;
    view?: { level?: string; headline?: string; rows?: unknown[]; buttons?: unknown[] } | null;
  }[];
  isLoading: boolean;
  isError: boolean;
};
let widgets: Widgets = { data: [], isLoading: false, isError: false };
const someWidgets = (...levels: string[]): Widgets => ({
  data: levels.map((level, i) => ({
    key: `w${i}`,
    view: { level, headline: `Widget ${i}`, rows: [], buttons: [] }
  })),
  isLoading: false,
  isError: false
});
const widgetsEnabled = vi.fn();
let hiddenCopies: { key: string }[] = [];
vi.mock("@/hooks/api/homeWidgets/useHomeWidgets", () => ({
  useHomeSections: (_org: string, enabled: boolean) => {
    widgetsEnabled(enabled);
    return sections;
  },
  useHomeWidgets: (_org: string, enabled: boolean) => {
    widgetsEnabled(enabled);
    return widgets;
  },
  useHiddenWidgets: () => hiddenCopies
}));
// The person's own choice: Today shown unless a test hides it.
let preferences: {
  data?: { hidden: string[]; today: "shown" | "hidden"; sections?: HomeSection["kind"][] };
  isPending: boolean;
} = {
  data: { hidden: [], today: "shown" },
  isPending: false
};
const setToday = vi.fn();
vi.mock("@/hooks/api/homeWidgets/useHomePreferences", () => ({
  useHomePreferences: () => preferences,
  useHomePreferenceActions: () => ({ setToday })
}));
// Wide (the rail) unless a test sets a phone.
let wide = true;
vi.mock("@/hooks/useMediaQuery", () => ({ useMediaQuery: () => wide }));
let recent: { id: string; title: string; timestamp: string }[] = [];
let recentSettled = true;
vi.mock("./components/HomeRecentQuestions", () => ({
  useRecentQuestions: () => ({ rows: recent, settled: recentSettled }),
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
  hiddenCopies = [];
  preferences = { data: { hidden: [], today: "shown" }, isPending: false };
  widgetsEnabled.mockReset();
  setToday.mockReset();
  wide = true;
  window.localStorage.removeItem("oxy.home.today.northwind");
  unread = {};
  latest = {};
  worldModel = { entities: [{ id: "store" }] };
  recent = [{ id: "t1", title: "Sales last week", timestamp: "2026-11-01T00:00:00Z" }];
  recentSettled = true;
  worldModelLoading = false;
  window.localStorage.removeItem("oxy.home.side.northwind");
  window.localStorage.removeItem("oxy.home.seen.northwind");
  window.sessionStorage.clear();
});

const appsOf = (n: number): CustomAppSummary[] =>
  Array.from({ length: n }, (_, i) => ({ ...revenue, id: `app-${i}`, slug: `app-${i}` }));
afterEach(cleanup);

describe("the home's frame", () => {
  it("puts Today, then recent questions and the data map, in a side column on a wide screen", () => {
    widgets = someWidgets("critical", "ok");
    renderHome();
    expect(screen.getByTestId("customer-home")).toHaveAttribute("data-today", "rail");
    const side = screen.getByTestId("home-side");
    expect(side.tagName).toBe("ASIDE");
    const today = screen.getByTestId("home-widgets-stub");
    expect(today).toHaveAttribute("data-variant", "rail");
    const lower = screen.getByTestId("home-lower-row");
    expect(side.contains(today) && side.contains(lower)).toBe(true);
    expect(today.compareDocumentPosition(lower) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    // The apps have the main column to themselves.
    expect(side.contains(screen.getByTestId("launcher-app-grid"))).toBe(false);
  });

  it("is one scroller: nothing inside the page scrolls on its own", () => {
    widgets = someWidgets("critical", "ok");
    renderHome();
    const home = screen.getByTestId("customer-home");
    expect(home).toHaveClass("overflow-y-auto");
    const inner = [...home.querySelectorAll("*")].filter((el) =>
      /\boverflow-(y-)?(auto|scroll)\b/.test(el.getAttribute("class") ?? "")
    );
    expect(inner).toEqual([]);
  });

  it("sizes the app grid by its own room, not the screen", () => {
    renderHome();
    const grid = screen.getByTestId("launcher-app-grid");
    expect(grid.closest(".\\@container")).not.toBeNull();
    expect(grid).toHaveClass("@sm:grid-cols-2", "@2xl:grid-cols-3", "@5xl:grid-cols-4");
  });

  it("puts the apps first and Today stacked under them on a narrow screen", () => {
    widgets = someWidgets("critical");
    wide = false;
    renderHome();
    expect(screen.queryByTestId("home-side")).toBeNull();
    const grid = screen.getByTestId("launcher-app-grid");
    const today = screen.getByTestId("home-widgets-stub");
    expect(today).toHaveAttribute("data-variant", "stack");
    expect(screen.getByTestId("customer-home")).toHaveAttribute("data-today", "stack");
    // The apps come before Today on the page, as they do on the screen.
    expect(grid.compareDocumentPosition(today) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("keeps the side column for recent questions and the data map when the org has no widgets", () => {
    renderHome();
    expect(screen.getByTestId("customer-home")).toHaveAttribute("data-today", "none");
    expect(screen.queryByTestId("home-widgets-stub")).toBeNull();
    expect(screen.getByTestId("home-side").contains(screen.getByTestId("home-lower-row"))).toBe(
      true
    );
  });

  it("draws the side column this browser saw last time while recent questions and the model load", () => {
    // No widgets: only the later reads can fill the side column.
    recentSettled = false;
    worldModelLoading = true;
    worldModel = undefined;
    recent = [];
    renderHome();
    // Never seen: the apps take the whole width, and nothing is remembered yet.
    expect(screen.queryByTestId("home-side")).toBeNull();
    expect(window.localStorage.getItem("oxy.home.side.northwind")).toBeNull();
    cleanup();
    window.localStorage.setItem("oxy.home.side.northwind", "1");
    renderHome();
    // Seen before: the grid has its final width from the first paint.
    expect(screen.getByTestId("home-side")).toBeInTheDocument();
  });

  it("remembers whether there was a side column once the reads are in", () => {
    renderHome();
    expect(window.localStorage.getItem("oxy.home.side.northwind")).toBe("1");
    cleanup();
    recent = [];
    worldModel = undefined;
    renderHome();
    expect(window.localStorage.getItem("oxy.home.side.northwind")).toBe("0");
    expect(screen.queryByTestId("home-side")).toBeNull();
  });

  it("gives the apps the whole width when the side column would be empty", () => {
    recent = [];
    worldModel = undefined;
    renderHome();
    expect(screen.queryByTestId("home-side")).toBeNull();
    expect(screen.queryByTestId("home-lower-row")).toBeNull();
  });

  it("draws the frame this browser saw last time while the widgets load", () => {
    widgets = { data: undefined, isLoading: true, isError: false };
    renderHome();
    // Never seen: no rail, so an org without widgets never sees one flash in.
    expect(screen.getByTestId("customer-home")).toHaveAttribute("data-today", "none");
    cleanup();
    window.localStorage.setItem("oxy.home.today.northwind", "1");
    renderHome();
    expect(screen.getByTestId("customer-home")).toHaveAttribute("data-today", "rail");
  });

  it("remembers what the widgets said, for the next visit's first paint", () => {
    widgets = someWidgets("ok");
    renderHome();
    expect(window.localStorage.getItem("oxy.home.today.northwind")).toBe("1");
    cleanup();
    widgets = { data: [], isLoading: false, isError: false };
    renderHome();
    expect(window.localStorage.getItem("oxy.home.today.northwind")).toBe("0");
    expect(screen.getByTestId("customer-home")).toHaveAttribute("data-today", "none");
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

  it("never says all clear when nothing answered, and names failures beside a clear day", () => {
    widgets = { data: [{ view: null }, { view: null }], isLoading: false, isError: false };
    renderHome();
    expect(screen.queryByTestId("home-status")).toBeNull();
    cleanup();
    widgets = {
      data: [...(someWidgets("ok").data ?? []), { key: "w1", view: null }],
      isLoading: false,
      isError: false
    };
    renderHome();
    expect(screen.getByTestId("home-status")).toHaveTextContent("All clear, but 1 couldn't load");
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
    expect(body.contains(screen.getByTestId("home-side"))).toBe(true);
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

describe("the home with Today hidden", () => {
  it("reads no widgets, draws no Today, and offers one quiet way back", () => {
    preferences = { data: { hidden: [], today: "hidden" }, isPending: false };
    widgets = someWidgets("critical");
    renderHome();
    // Not one widgets read is allowed: both queries are told they are off.
    expect(widgetsEnabled).toHaveBeenCalled();
    expect(widgetsEnabled.mock.calls.every(([enabled]) => enabled === false)).toBe(true);
    expect(screen.getByTestId("customer-home")).toHaveAttribute("data-today", "none");
    expect(screen.queryByTestId("home-widgets-stub")).toBeNull();
    expect(screen.queryByTestId("home-status")).toBeNull();
    // The apps stay.
    expect(screen.getByRole("heading", { name: "Your apps" })).toBeInTheDocument();
    fireEvent.click(screen.getByTestId("home-today-show"));
    expect(setToday).toHaveBeenCalledWith("shown");
  });

  it("keeps Insights out when the org layout dropped it, though the widgets are not read", () => {
    // The disabled sections query answers the default, which has Insights.
    preferences = { data: { hidden: [], today: "hidden", sections: ["apps"] }, isPending: false };
    renderHome();
    expect(widgetsEnabled.mock.calls.every(([enabled]) => enabled === false)).toBe(true);
    expect(screen.getByTestId("launcher-app-grid")).toBeInTheDocument();
    expect(screen.queryByTestId("home-lower-row")).toBeNull();
  });

  it("keeps Insights when the layout has it, and falls back to the default when none was named", () => {
    preferences = {
      data: { hidden: [], today: "hidden", sections: ["apps", "insights"] },
      isPending: false
    };
    renderHome();
    expect(screen.getByTestId("home-lower-row")).toBeInTheDocument();
    cleanup();
    preferences = { data: { hidden: [], today: "hidden" }, isPending: false };
    renderHome();
    expect(screen.getByTestId("home-lower-row")).toBeInTheDocument();
  });

  it("waits for the preference before reading widgets, and shows no way back yet", () => {
    preferences = { data: undefined, isPending: true };
    renderHome();
    expect(widgetsEnabled.mock.calls.every(([enabled]) => enabled === false)).toBe(true);
    expect(screen.queryByTestId("home-today-show")).toBeNull();
  });

  it("reads the widgets once Today is shown, and offers no Show Today", () => {
    widgets = someWidgets("critical");
    renderHome();
    expect(widgetsEnabled.mock.calls.every(([enabled]) => enabled === true)).toBe(true);
    expect(screen.getByTestId("home-widgets-stub")).toHaveAttribute("data-enabled", "true");
    expect(screen.queryByTestId("home-today-show")).toBeNull();
  });

  it("still has a Today when every copy is hidden, so there is a way to bring them back", () => {
    widgets = { data: [], isLoading: false, isError: false };
    hiddenCopies = [{ key: "bookkeeping/close" }];
    renderHome();
    expect(screen.getByTestId("customer-home")).toHaveAttribute("data-today", "rail");
  });
});

describe("opening an app", () => {
  // jsdom cannot navigate; the browser would leave the page here.
  const stay = (event: Event) => event.preventDefault();
  beforeEach(() => window.addEventListener("click", stay));
  afterEach(() => window.removeEventListener("click", stay));

  it("grows the tile into the app's launch screen and leaves the app a note to finish it", () => {
    renderHome();
    fireEvent.click(screen.getByTestId("launcher-app-card-revenue"));
    const launch = screen.getByTestId("app-launch");
    expect(launch).toHaveAttribute("data-app", "revenue");
    expect(launch).toHaveTextContent("Revenue Board");
    const note = JSON.parse(window.sessionStorage.getItem("oxy.launch") ?? "null");
    expect(note).toMatchObject({
      root: "/customer-apps/northwind/revenue/",
      name: "Revenue Board",
      mark: "RB"
    });
    expect(Date.now() - note.at).toBeLessThan(1000);
  });

  it("gives the home back, and takes the note back, when Back restores the page", () => {
    renderHome();
    fireEvent.click(screen.getByTestId("launcher-app-card-revenue"));
    act(() => {
      window.dispatchEvent(Object.assign(new Event("pageshow"), { persisted: true }));
    });
    expect(screen.queryByTestId("app-launch")).toBeNull();
    expect(window.sessionStorage.getItem("oxy.launch")).toBeNull();
  });

  it("leaves a press that opens a new tab alone", () => {
    renderHome();
    fireEvent.click(screen.getByTestId("launcher-app-card-revenue"), { metaKey: true });
    expect(screen.queryByTestId("app-launch")).toBeNull();
    expect(window.sessionStorage.getItem("oxy.launch")).toBeNull();
  });
});

describe("since the last visit", () => {
  it("says how many widgets changed while the person was away", () => {
    widgets = someWidgets("ok", "ok");
    window.localStorage.setItem(
      "oxy.home.seen.northwind",
      JSON.stringify({ w0: "something-else", w1: "also-different" })
    );
    renderHome();
    expect(screen.getByTestId("home-status-since")).toHaveTextContent(
      "2 updates since your last visit"
    );
  });

  it("says nothing on a first visit, and remembers this one for the next", () => {
    widgets = someWidgets("ok");
    const { unmount } = renderHome();
    expect(screen.queryByTestId("home-status-since")).toBeNull();
    unmount();
    expect(
      JSON.parse(window.localStorage.getItem("oxy.home.seen.northwind") ?? "{}")
    ).toHaveProperty("w0");
  });
});
