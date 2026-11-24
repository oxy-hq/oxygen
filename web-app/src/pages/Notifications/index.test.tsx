// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AxiosError, type AxiosResponse } from "axios";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { NotificationsService } from "@/services/api/notifications";
import type { InboxItem, InboxResponse, NotificationCatalog } from "@/types/notifications";

vi.mock("@/services/api/notifications", () => ({
  NotificationsService: {
    catalog: vi.fn(),
    inbox: vi.fn(),
    markRead: vi.fn(),
    markAllRead: vi.fn(),
    runAction: vi.fn(),
    unread: vi.fn(),
    vapidPublicKey: vi.fn()
  }
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

import {
  fakePushBrowser,
  noPushBrowser
} from "@/components/Notifications/PushPrompt/__tests__/fakePushBrowser";
import NotificationsPage from ".";
import { WIDE_MEDIA_QUERY } from "./constants";

const service = vi.mocked(NotificationsService);
const NOW = new Date("2026-10-14T12:00:00Z");

const STORE_OPS = { id: "app-1", slug: "store-ops", name: "Store Ops" };
const FOOD_SAFETY = {
  path: "operations/food-safety",
  labels: ["Store operations", "Food safety"]
};

const item = (id: string, over: Partial<InboxItem> = {}): InboxItem => ({
  id,
  org_id: "org-1",
  kind: "list_overdue",
  created_at: "2026-10-14T11:55:00Z",
  read_at: null,
  title: `Title ${id}`,
  body: null,
  href: "/customer-apps/acme/store-ops/lists/42",
  link: "/lists/42",
  app: STORE_OPS,
  category: { path: "operations", labels: ["Store operations"] },
  kind_label: "Overdue lists",
  level: "active",
  thread: null,
  actions: [],
  url: null,
  ...over
});

const alarm = (id: string, created_at: string) =>
  item(id, {
    kind: "temp_alarm",
    created_at,
    category: FOOD_SAFETY,
    level: "critical",
    thread: "t-cooler",
    actions: [{ id: "ack", label: "Acknowledge", destructive: false, runs_function: true }]
  });

const oxygenItem = item("n-oxy", {
  kind: "oxy.run_failed",
  created_at: "2026-10-13T09:00:00Z",
  title: "Nightly sync failed",
  href: "/acme/workspaces/ws-1/automations",
  link: null,
  app: null,
  category: { path: "runs", labels: ["Automations and pipelines"] },
  level: "time_sensitive",
  read_at: "2026-10-13T10:00:00Z",
  actions: [{ id: "open", label: "Open run", destructive: false, runs_function: false }]
});

const page = (items: InboxItem[], over: Partial<InboxResponse> = {}): InboxResponse => ({
  unread: items.filter((i) => i.read_at === null).length,
  items,
  next_before: null,
  ...over
});

const firstPage = page(
  [
    alarm("n-alarm-3", "2026-10-14T11:58:00Z"),
    item("n-list"),
    alarm("n-alarm-2", "2026-10-14T11:30:00Z"),
    alarm("n-alarm-1", "2026-10-14T11:00:00Z"),
    oxygenItem
  ],
  { next_before: "n-oxy" }
);

const catalog: NotificationCatalog = {
  reach: { push_devices: 1, email: true },
  sources: [
    {
      source: "app",
      app: { ...STORE_OPS, org_slug: "acme" },
      label: "Store Ops",
      muted: [],
      unread: 4,
      categories: [
        {
          path: "operations",
          label: "Store operations",
          description: null,
          muted: [],
          unread: 4,
          kinds: [
            {
              kind: "list_overdue",
              label: "Overdue lists",
              description: null,
              level: "time_sensitive",
              critical: false,
              channels: ["push", "email", "slack"],
              muted: [],
              actions: [],
              unread: 1
            }
          ],
          categories: [
            {
              path: "operations/food-safety",
              label: "Food safety",
              description: null,
              muted: [],
              unread: 3,
              categories: [],
              kinds: [
                {
                  kind: "temp_alarm",
                  label: "Temperature alarm",
                  description: null,
                  level: "critical",
                  critical: true,
                  channels: ["push", "email", "slack"],
                  muted: [],
                  actions: [],
                  unread: 3
                }
              ]
            }
          ]
        }
      ]
    },
    {
      source: "oxygen",
      app: null,
      label: "Oxygen",
      muted: [],
      unread: 0,
      categories: [
        {
          path: "runs",
          label: "Automations and pipelines",
          description: null,
          muted: [],
          unread: 0,
          categories: [],
          kinds: []
        }
      ]
    }
  ]
};

function Where() {
  const location = useLocation();
  return <p data-testid='where'>{`${location.pathname}${location.search}`}</p>;
}

function renderPage(url = "/notifications") {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } }
  });
  render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter initialEntries={[url]}>
        <Routes>
          <Route
            path='*'
            element={
              <>
                <NotificationsPage />
                <Where />
              </>
            }
          />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>
  );
}

/** A phone: the wide layout's `min-width` query stops matching. */
function onAPhone() {
  vi.spyOn(window, "matchMedia").mockImplementation(
    (query: string) =>
      ({
        matches: query !== WIDE_MEDIA_QUERY,
        media: query,
        onchange: null,
        addEventListener: () => {},
        removeEventListener: () => {},
        addListener: () => {},
        removeListener: () => {},
        dispatchEvent: () => false
      }) as unknown as MediaQueryList
  );
}

const lastInboxCall = () => service.inbox.mock.calls.at(-1)?.[0];
const stopNavigation = (event: MouseEvent) => event.preventDefault();

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(NOW);
  document.addEventListener("click", stopNavigation);
  service.catalog.mockResolvedValue(catalog);
  service.inbox.mockResolvedValue(firstPage);
  service.markRead.mockResolvedValue();
  service.markAllRead.mockResolvedValue();
  service.runAction.mockResolvedValue({ ok: true, result: null });
  // The page reads the badge for its stream counts; the tabs are their own
  // file (`streams.test.tsx`), so here it only has to answer.
  service.unread.mockResolvedValue({
    unread: 4,
    apps: [],
    streams: { activity: 4, operations: 0 }
  });
  service.vapidPublicKey.mockResolvedValue({ configured: false, public_key: null });
});

afterEach(() => {
  cleanup();
  document.removeEventListener("click", stopNavigation);
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.clearAllMocks();
});

describe("Notifications page list", () => {
  it("groups by day, newest first, and collapses a thread into its newest item", async () => {
    renderPage();
    const list = await screen.findByTestId("notifications-page-list");
    const days = within(list).getAllByRole("region");
    expect(days.map((d) => d.getAttribute("aria-label"))).toEqual(["Today", "Yesterday"]);
    const today = within(days[0]).getAllByRole("listitem");
    expect(today.map((r) => r.getAttribute("data-testid"))).toEqual([
      "notifications-row-n-alarm-3",
      "notifications-row-n-list"
    ]);
    expect(within(today[0]).getByTestId("notifications-thread-more-n-alarm-3")).toHaveTextContent(
      "+2 more"
    );
    expect(screen.queryByTestId("notifications-row-n-alarm-1")).toBeNull();
    expect(screen.queryByTestId("notifications-thread-more-n-list")).toBeNull();
  });

  it("says where each came from and marks critical and time-sensitive ones", async () => {
    renderPage();
    const alarmRow = await screen.findByTestId("notifications-item-n-alarm-3");
    expect(alarmRow).toHaveTextContent("Store Ops · Food safety");
    expect(screen.getByTestId("notifications-row-n-alarm-3")).toHaveClass("border-l-destructive");
    expect(screen.getByTestId("notifications-item-n-oxy")).toHaveTextContent(
      "Oxygen · Automations and pipelines"
    );
    expect(screen.getByTestId("notifications-row-n-oxy")).toHaveClass("border-l-warning");
    expect(screen.getByTestId("notifications-row-n-list")).not.toHaveClass("border-l-destructive");
  });

  it("asks for everything by default, and only unread on Unread", async () => {
    renderPage();
    await screen.findByTestId("notifications-page-list");
    expect(lastInboxCall()).toEqual({ unreadOnly: false, limit: 50, before: undefined });
    await userEvent.click(screen.getByTestId("notifications-page-unread"));
    await waitFor(() => expect(lastInboxCall()).toMatchObject({ unreadOnly: true }));
    expect(screen.getByTestId("where")).toHaveTextContent("unread=1");
  });

  it("opens a whole thread from +N more, and goes back from its chip", async () => {
    renderPage();
    await userEvent.click(await screen.findByTestId("notifications-thread-more-n-alarm-3"));
    await waitFor(() =>
      expect(lastInboxCall()).toMatchObject({ source: "app-1", thread: "t-cooler" })
    );
    service.inbox.mockResolvedValue(
      page([
        alarm("n-alarm-3", "2026-10-14T11:58:00Z"),
        alarm("n-alarm-2", "2026-10-14T11:30:00Z"),
        alarm("n-alarm-1", "2026-10-14T11:00:00Z")
      ])
    );
    // Inside one thread nothing collapses.
    expect(await screen.findByTestId("notifications-row-n-alarm-1")).toBeInTheDocument();
    expect(screen.queryByTestId("notifications-thread-more-n-alarm-3")).toBeNull();

    await userEvent.click(screen.getByTestId("notifications-thread-clear"));
    await waitFor(() => expect(lastInboxCall()).not.toHaveProperty("thread"));
  });

  it("pages back through history with the before cursor", async () => {
    renderPage();
    await screen.findByTestId("notifications-page-list");
    service.inbox.mockResolvedValueOnce(
      page([item("n-old", { created_at: "2026-10-01T09:00:00Z", read_at: NOW.toISOString() })])
    );
    await userEvent.click(screen.getByTestId("notifications-page-load-more"));
    await waitFor(() => expect(lastInboxCall()).toMatchObject({ before: "n-oxy" }));
    expect(await screen.findByTestId("notifications-row-n-old")).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Thu, Oct 1" })).toBeInTheDocument();
    // The last page said there is nothing older.
    expect(screen.queryByTestId("notifications-page-load-more")).toBeNull();
  });

  it("shows a server's items that carry none of the new fields, with nothing extra", async () => {
    const old: InboxItem = {
      id: "n-legacy",
      org_id: "org-1",
      kind: "list_overdue",
      created_at: "2026-10-14T11:00:00Z",
      read_at: null,
      title: "Legacy",
      body: null,
      href: null,
      link: null,
      app: STORE_OPS
    };
    service.inbox.mockResolvedValue({ unread: 1, items: [old] });
    renderPage();
    const row = await screen.findByTestId("notifications-row-n-legacy");
    expect(within(row).getByTestId("notifications-item-source")).toHaveTextContent(/^Store Ops$/);
    expect(within(row).queryAllByRole("button")).toHaveLength(1);
    expect(screen.queryByTestId("notifications-page-load-more")).toBeNull();
  });

  it("says so when there is nothing unread", async () => {
    service.inbox.mockResolvedValue(page([]));
    renderPage("/notifications?unread=1");
    expect(await screen.findByTestId("notifications-page-empty")).toHaveTextContent(
      "You're all caught up"
    );
  });
});

describe("Notifications page tree and filters", () => {
  it("lists sources, categories and kinds with unread counts, Oxygen first", async () => {
    renderPage();
    const tree = await screen.findByTestId("notifications-tree");
    await within(tree).findByTestId("notifications-tree-oxygen");
    const nodes = within(tree).getAllByRole("button");
    expect(nodes.map((n) => n.getAttribute("data-testid"))).toEqual([
      "notifications-tree-all",
      "notifications-tree-oxygen",
      "notifications-tree-oxygen-cat-runs",
      "notifications-tree-store-ops",
      "notifications-tree-store-ops-cat-operations",
      "notifications-tree-store-ops-kind-list_overdue",
      "notifications-tree-store-ops-cat-operations/food-safety",
      "notifications-tree-store-ops-kind-temp_alarm"
    ]);
    expect(within(tree).getByTestId("notifications-tree-all")).toHaveTextContent("All4");
    expect(
      within(tree).getByTestId("notifications-tree-store-ops-cat-operations/food-safety")
    ).toHaveTextContent("Food safety3");
    expect(within(tree).getByTestId("notifications-tree-oxygen")).toHaveTextContent(/^Oxygen$/);
  });

  it("narrows the list to a category's subtree, and says so", async () => {
    renderPage();
    await userEvent.click(await screen.findByTestId("notifications-tree-store-ops-cat-operations"));
    await waitFor(() =>
      expect(lastInboxCall()).toEqual({
        source: "app-1",
        category: "operations",
        unreadOnly: false,
        limit: 50,
        before: undefined
      })
    );
    expect(screen.getByTestId("notifications-page-scope")).toHaveTextContent(
      "Store Ops · Store operations"
    );
    expect(screen.getByTestId("notifications-tree-store-ops-cat-operations")).toHaveAttribute(
      "aria-current",
      "true"
    );
    expect(screen.getByTestId("where")).toHaveTextContent(
      "/notifications?source=app-1&category=operations"
    );
  });

  it("narrows to one kind of one source", async () => {
    renderPage();
    await userEvent.click(
      await screen.findByTestId("notifications-tree-store-ops-kind-temp_alarm")
    );
    await waitFor(() =>
      expect(lastInboxCall()).toMatchObject({ source: "app-1", kind: "temp_alarm" })
    );
    expect(lastInboxCall()).not.toHaveProperty("category");
  });

  it("marks read only what the filter covers", async () => {
    renderPage("/notifications?source=app-1&category=operations/food-safety");
    const button = await screen.findByTestId("notifications-page-mark-all-read");
    await waitFor(() => expect(button).toBeEnabled());
    await userEvent.click(button);
    await waitFor(() =>
      expect(service.markAllRead).toHaveBeenCalledWith({
        source: "app-1",
        category: "operations/food-safety"
      })
    );
  });

  it("has nothing to mark when the filtered list is all read", async () => {
    service.inbox.mockResolvedValue(page([oxygenItem]));
    renderPage("/notifications?source=oxygen");
    await screen.findByTestId("notifications-row-n-oxy");
    expect(screen.getByTestId("notifications-page-mark-all-read")).toBeDisabled();
  });
});

describe("Notifications page actions", () => {
  it("runs a function action, busy while it runs, then says it is done", async () => {
    let answer!: () => void;
    service.runAction.mockReturnValue(
      new Promise((resolve) => {
        answer = () => resolve({ ok: true, result: { acknowledged: true } });
      })
    );
    renderPage();
    const button = await screen.findByTestId("notifications-action-n-alarm-3-ack");
    expect(button).toHaveTextContent("Acknowledge");
    await userEvent.click(button);
    await waitFor(() => expect(button).toBeDisabled());
    expect(service.runAction).toHaveBeenCalledWith("n-alarm-3", "ack");
    const alarms = screen.getByTestId("notifications-tree-store-ops-kind-temp_alarm");
    expect(alarms).toHaveTextContent(/3$/);
    answer();
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Done"));
    await waitFor(() => expect(button).toBeEnabled());
    // The server marked it read: the list is read again, and the tree's counts
    // drop in place — the catalogue itself is not re-read.
    expect(service.inbox.mock.calls.length).toBeGreaterThan(1);
    await waitFor(() => expect(alarms).toHaveTextContent(/2$/));
    expect(screen.getByTestId("notifications-tree-store-ops")).toHaveTextContent(/3$/);
    expect(service.catalog).toHaveBeenCalledTimes(1);
  });

  it("says the server's words when the app's function refused, and keeps the count", async () => {
    service.runAction.mockRejectedValue(
      new AxiosError("Forbidden", "ERR_BAD_REQUEST", undefined, undefined, {
        status: 403,
        data: {
          ok: false,
          error: "FunctionRefused",
          message: "the app's function did not accept your sign-in"
        }
      } as AxiosResponse)
    );
    renderPage();
    await userEvent.click(await screen.findByTestId("notifications-action-n-alarm-3-ack"));
    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith("the app's function did not accept your sign-in")
    );
    expect(screen.getByTestId("notifications-tree-store-ops-kind-temp_alarm")).toHaveTextContent(
      /3$/
    );
  });

  it("says an action is gone when the app no longer declares it", async () => {
    service.runAction.mockRejectedValue(
      new AxiosError("Conflict", "ERR_BAD_REQUEST", undefined, undefined, {
        status: 409,
        data: { message: "not declared" }
      } as AxiosResponse)
    );
    renderPage();
    await userEvent.click(await screen.findByTestId("notifications-action-n-alarm-3-ack"));
    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith("This action is no longer available")
    );
    expect(toast.success).not.toHaveBeenCalled();
  });

  it("passes on the server's own words for any other refusal", async () => {
    service.runAction.mockRejectedValue(
      new AxiosError("Forbidden", "ERR_BAD_REQUEST", undefined, undefined, {
        status: 403,
        data: { message: "Not on a staging host" }
      } as AxiosResponse)
    );
    renderPage();
    await userEvent.click(await screen.findByTestId("notifications-action-n-alarm-3-ack"));
    await waitFor(() => expect(toast.error).toHaveBeenCalledWith("Not on a staging host"));
  });

  it("makes an open action a link to the notification, which needs no call", async () => {
    renderPage();
    const open = await screen.findByTestId("notifications-action-n-oxy-open");
    expect(open).toHaveTextContent("Open run");
    expect(open).toHaveAttribute("href", "/acme/workspaces/ws-1/automations");
    await userEvent.click(open);
    expect(service.runAction).not.toHaveBeenCalled();
    // Already read: nothing to mark.
    expect(service.markRead).not.toHaveBeenCalled();
  });
});

describe("Notifications page on a phone", () => {
  beforeEach(onAPhone);

  it("fills the screen with the list and puts the tree in a filter sheet", async () => {
    renderPage();
    await screen.findByTestId("notifications-page-list");
    expect(screen.queryByTestId("notifications-page-sidebar")).toBeNull();
    expect(screen.queryByTestId("notifications-tree")).toBeNull();
    const open = screen.getByTestId("notifications-filter-open");
    expect(open).toHaveTextContent("All notifications");

    await userEvent.click(open);
    const sheet = await screen.findByTestId("notifications-filter-sheet");
    expect(screen.getByRole("dialog", { name: "Filter" })).toBe(sheet);
    await userEvent.click(
      await within(sheet).findByTestId("notifications-tree-store-ops-kind-temp_alarm")
    );
    await waitFor(() => expect(screen.queryByTestId("notifications-filter-sheet")).toBeNull());
    await waitFor(() =>
      expect(lastInboxCall()).toMatchObject({ source: "app-1", kind: "temp_alarm" })
    );
    expect(screen.getByTestId("notifications-filter-open")).toHaveTextContent(
      "Store Ops · Temperature alarm"
    );
  });

  it("keeps Unread / All and Mark all read beside the filter", async () => {
    renderPage();
    await screen.findByTestId("notifications-page-list");
    expect(screen.getByTestId("notifications-page-unread")).toBeInTheDocument();
    expect(screen.getByTestId("notifications-page-mark-all-read")).toHaveAccessibleName(
      "Mark all read"
    );
  });
});

describe("Notifications page and push", () => {
  afterEach(noPushBrowser);
  const PUBLIC_KEY = `B${"A".repeat(86)}`;

  it("offers push on this device above the list", async () => {
    fakePushBrowser();
    service.vapidPublicKey.mockResolvedValue({ configured: true, public_key: PUBLIC_KEY });
    renderPage();
    expect(await screen.findByTestId("push-prompt")).toHaveTextContent(
      "Get notifications on this device"
    );
  });

  it("offers it on a phone too", async () => {
    onAPhone();
    fakePushBrowser();
    service.vapidPublicKey.mockResolvedValue({ configured: true, public_key: PUBLIC_KEY });
    renderPage();
    expect(await screen.findByTestId("push-prompt-turn-on")).toBeInTheDocument();
  });

  it("goes where a clicked push says: the page has no bell to do it", async () => {
    const worker = new EventTarget();
    Object.defineProperty(navigator, "serviceWorker", { value: worker, configurable: true });
    renderPage();
    await screen.findByTestId("notifications-page");
    const href = "/notifications?source=app-1&kind=temp_alarm";
    act(() => {
      worker.dispatchEvent(
        new MessageEvent("message", { data: { type: "oxy:open-notifications", href } })
      );
    });
    await waitFor(() => expect(screen.getByTestId("where")).toHaveTextContent(href));
  });
});
