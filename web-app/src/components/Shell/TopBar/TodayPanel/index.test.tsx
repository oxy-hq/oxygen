// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import queryKeys from "@/hooks/api/queryKey";
import ROUTES from "@/libs/utils/routes";
import type { HomeWidgetsResponse, WidgetCard } from "@/types/homeWidgets";
import { WorkspaceClock } from "../WorkspaceClock";

const service = vi.hoisted(() => ({ list: vi.fn() }));
vi.mock("@/services/api/homeWidgets", () => ({ HomeWidgetsService: service }));
vi.mock("@/hooks/api/agents/useWorkspaceTimezone", () => ({
  default: () => "America/Los_Angeles"
}));
vi.mock("@/stores/useCurrentOrg", () => ({
  default: <T,>(selector: (s: unknown) => T) =>
    selector({ org: { id: "org-1", slug: "poke-house", name: "Poke House" } })
}));
vi.mock("@/stores/useCurrentWorkspace", () => ({
  default: <T,>(selector: (s: unknown) => T) => selector({ workspace: { id: "ws-1" } })
}));
vi.mock("@/hooks/api/customApps/useOrgCustomApps", () => ({
  default: () => ({ apps: [{ id: "app-1", icon_url: null }], isPending: false })
}));

const BASE = "https://poke-house--bookkeeping.apps.test";
const copy = (id: string, over: Partial<WidgetCard> = {}): WidgetCard => ({
  app: {
    id: "app-1",
    slug: "bookkeeping",
    name: "Bookkeeping",
    org_slug: "poke-house",
    url: `${BASE}/`
  },
  key: `bookkeeping/${id}`,
  id,
  instance: null,
  title: `Widget ${id}`,
  params: {},
  size: "regular",
  sizes: ["regular"],
  view: {
    headline: `${id} headline`,
    detail: null,
    level: "attention",
    progress: null,
    metric: null,
    rows: [],
    buttons: [],
    href: `${BASE}/${id}`,
    badge: null,
    refresh_after: null
  },
  error: null,
  ...over
});
const withView = (id: string, view: Partial<NonNullable<WidgetCard["view"]>>): WidgetCard => {
  const base = copy(id);
  return { ...base, view: { ...(base.view as NonNullable<WidgetCard["view"]>), ...view } };
};
const calm = (id: string) => withView(id, { level: "ok" });

const answer = (widgets: WidgetCard[], over: Partial<HomeWidgetsResponse> = {}) =>
  service.list.mockResolvedValue({ widgets, sections: [], hidden: [], today: "shown", ...over });

let qc: QueryClient;
const renderClock = () =>
  render(
    <QueryClientProvider client={qc}>
      <MemoryRouter>
        <WorkspaceClock withDate />
      </MemoryRouter>
    </QueryClientProvider>
  );
const open = async () => {
  await userEvent.click(screen.getByTestId("topbar-clock"));
  return screen.findByTestId("topbar-today");
};
const rows = (panel: HTMLElement) =>
  within(panel)
    .queryAllByTestId(/^topbar-today-row-bookkeeping-[a-z]+$/)
    .map((el) => el.getAttribute("data-testid")?.replace("topbar-today-row-bookkeeping-", ""));

beforeEach(() => {
  qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  // Pinned: the clock reads `new Date()`.
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(new Date("2026-10-29T22:42:00Z"));
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.clearAllMocks();
});

describe("Today from the top bar", () => {
  it("opens from the clock, and reads nothing until it does", async () => {
    answer([copy("close")]);
    renderClock();
    expect(screen.queryByTestId("topbar-today")).toBeNull();
    expect(service.list).not.toHaveBeenCalled();
    const panel = await open();
    expect(service.list).toHaveBeenCalledWith("poke-house");
    expect(
      await within(panel).findByTestId("topbar-today-row-bookkeeping-close")
    ).toBeInTheDocument();
    expect(screen.getByTestId("topbar-clock")).toHaveAccessibleName(/^Today, /);
  });

  it("puts what needs the person first, critical before attention, then calm, then couldn't load last", async () => {
    answer([
      calm("fine"),
      copy("broken", { view: null, error: "Timed out." }),
      copy("warn"),
      withView("urgent", { level: "critical" })
    ]);
    renderClock();
    const panel = await open();
    await within(panel).findByTestId("topbar-today-row-bookkeeping-warn");
    expect(rows(panel)).toEqual(["urgent", "warn", "fine", "broken"]);
    expect(within(panel).getByTestId("topbar-today-calm")).toHaveTextContent("All clear");
    expect(within(panel).getByTestId("topbar-today-failed")).toHaveTextContent("Couldn't load");
    expect(
      within(panel).getByTestId("topbar-today-row-bookkeeping-broken-headline")
    ).toHaveTextContent("Couldn't load");
  });

  it("makes each row a link to the view, else the app, with a count only where someone has a job", async () => {
    answer([
      withView("close", { level: "attention", progress: { done: 61, total: 84 } }),
      withView("store", { level: "ok", progress: { done: 0, total: 17 } }),
      copy("bare", {
        view: { ...(copy("bare").view as NonNullable<WidgetCard["view"]>), href: null }
      })
    ]);
    renderClock();
    const panel = await open();
    const close = await within(panel).findByTestId("topbar-today-row-bookkeeping-close");
    expect(close).toHaveAttribute("href", `${BASE}/close`);
    expect(close).toHaveAccessibleName("Bookkeeping · Widget close: close headline");
    expect(
      within(close).getByTestId("topbar-today-row-bookkeeping-close-progress")
    ).toHaveTextContent("61/84");
    expect(within(panel).getByTestId("topbar-today-row-bookkeeping-bare")).toHaveAttribute(
      "href",
      `${BASE}/`
    );
    expect(within(panel).queryByTestId("topbar-today-row-bookkeeping-bare-progress")).toBeNull();
    // A calm row keeps its count to itself: "0/17" beside "on time" is noise.
    expect(within(panel).queryByTestId("topbar-today-row-bookkeeping-store-progress")).toBeNull();
  });

  it("ends in Open home, which goes to the org's home and closes the panel", async () => {
    answer([copy("close")]);
    renderClock();
    const panel = await open();
    const home = within(panel).getByTestId("topbar-today-home");
    expect(home).toHaveTextContent("Open home");
    expect(home).toHaveAttribute("href", ROUTES.ORG("poke-house").HOME);
    await userEvent.click(home);
    expect(screen.queryByTestId("topbar-today")).toBeNull();
  });

  it("says so when there is nothing for the person", async () => {
    answer([]);
    renderClock();
    const panel = await open();
    expect(await within(panel).findByTestId("topbar-today-empty")).toHaveTextContent(
      "Nothing for you today."
    );
  });

  it("shows Today whatever the person chose for their home, and leaves hidden copies out", async () => {
    answer([copy("close")], {
      today: "hidden",
      hidden: [
        {
          key: "bookkeeping/payroll",
          title: "Payroll",
          app: copy("payroll").app
        }
      ]
    });
    renderClock();
    const panel = await open();
    await within(panel).findByTestId("topbar-today-row-bookkeeping-close");
    expect(within(panel).queryByText("Payroll")).toBeNull();
    expect(rows(panel)).toEqual(["close"]);
  });

  it("shares the home's cached request instead of asking again", async () => {
    const cached: HomeWidgetsResponse = {
      widgets: [copy("close")],
      sections: [],
      hidden: [],
      today: "shown"
    };
    qc.setQueryData(queryKeys.homeWidgets.list("poke-house"), cached);
    renderClock();
    const panel = await open();
    expect(within(panel).getByTestId("topbar-today-row-bookkeeping-close")).toBeInTheDocument();
    expect(service.list).not.toHaveBeenCalled();
  });

  it("offers a retry when it could not load", async () => {
    service.list.mockRejectedValueOnce(new Error("down"));
    renderClock();
    const panel = await open();
    expect(await within(panel).findByTestId("topbar-today-error")).toHaveTextContent(
      "Couldn't load Today."
    );
    answer([copy("close")]);
    await userEvent.click(within(panel).getByTestId("topbar-today-retry"));
    expect(
      await within(panel).findByTestId("topbar-today-row-bookkeeping-close")
    ).toBeInTheDocument();
  });
});
