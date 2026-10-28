// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import queryKeys from "@/hooks/api/queryKey";
import { NotificationsService } from "@/services/api/notifications";
import type { InboxItem } from "@/types/notifications";

vi.mock("@/services/api/notifications", () => ({
  NotificationsService: {
    unread: vi.fn(),
    inbox: vi.fn(),
    markRead: vi.fn(),
    markAllRead: vi.fn(),
    vapidPublicKey: vi.fn()
  }
}));
vi.mock("@/contexts/AuthContext", () => ({ useAuth: () => ({ isLocalMode: false }) }));

import {
  fakePushBrowser,
  noPushBrowser
} from "@/components/Notifications/PushPrompt/__tests__/fakePushBrowser";
import { NotificationsBell } from ".";
import { POPOVER_MEDIA_QUERY } from "./constants";

const service = vi.mocked(NotificationsService);

const NOW = new Date("2026-10-14T12:00:00Z");

const item = (over: Partial<InboxItem> = {}): InboxItem => ({
  id: "n-app",
  org_id: "org-1",
  kind: "list_overdue",
  created_at: "2026-10-14T11:55:00Z",
  read_at: null,
  title: "Walk-in cooler list is overdue",
  body: "3 of 12 checks are still open.",
  href: "/customer-apps/acme/store-ops/lists/42",
  link: "/lists/42",
  app: { id: "app-1", slug: "store-ops", name: "Store Ops" },
  ...over
});

const platformItem = item({
  id: "n-platform",
  kind: "oxy.workspace_ready",
  title: "Your workspace is ready",
  body: null,
  href: "/acme/workspaces/ws-1/threads",
  link: null,
  app: null,
  read_at: "2026-10-14T11:00:00Z"
});

const criticalItem = item({
  id: "n-critical",
  kind: "temp_alarm",
  title: "Walk-in cooler is at 9°C",
  body: null,
  level: "critical",
  kind_label: "Temperature alarm",
  category: { path: "operations/food-safety", labels: ["Store operations", "Food safety"] },
  thread: "t-cooler",
  actions: [{ id: "ack", label: "Acknowledge", destructive: false, runs_function: true }]
});

const unreadableItem = item({
  id: "n-unreadable",
  title: null,
  body: null,
  href: null,
  link: null
});

/** A phone-width screen: the popover's `min-width` query stops matching. */
function onAPhone() {
  vi.spyOn(window, "matchMedia").mockImplementation(
    (query: string) =>
      ({
        matches: query !== POPOVER_MEDIA_QUERY,
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

function Where() {
  const location = useLocation();
  return <p data-testid='where'>{`${location.pathname}${location.search}`}</p>;
}

function renderBell(url = "/acme/workspaces/ws-1/home") {
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
                <NotificationsBell />
                <Where />
              </>
            }
          />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>
  );
  return queryClient;
}

const openInbox = async () => {
  await userEvent.click(screen.getByTestId("notifications-bell"));
  return screen.findByTestId("notifications-panel");
};

// jsdom cannot navigate a document; the anchors' default action is not what is
// under test, only where they point and what the click does first.
const stopNavigation = (event: MouseEvent) => event.preventDefault();

beforeEach(() => {
  // Only `Date` is faked: React Query and Testing Library keep real timers.
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(NOW);
  document.addEventListener("click", stopNavigation);
  service.unread.mockResolvedValue({ unread: 2 });
  service.inbox.mockResolvedValue({ unread: 2, items: [item(), platformItem, unreadableItem] });
  service.markRead.mockResolvedValue();
  service.markAllRead.mockResolvedValue();
  service.vapidPublicKey.mockResolvedValue({ configured: false, public_key: null });
});

afterEach(() => {
  cleanup();
  document.removeEventListener("click", stopNavigation);
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.clearAllMocks();
});

describe("NotificationsBell badge", () => {
  it("is hidden when nothing is unread", async () => {
    service.unread.mockResolvedValue({ unread: 0 });
    const queryClient = renderBell();
    await waitFor(() =>
      expect(queryClient.getQueryState(queryKeys.notifications.unread())?.status).toBe("success")
    );
    expect(screen.queryByTestId("notifications-badge")).toBeNull();
    expect(screen.getByTestId("notifications-bell")).toHaveAccessibleName("Notifications");
  });

  it("shows the unread count", async () => {
    service.unread.mockResolvedValue({ unread: 7 });
    renderBell();
    expect(await screen.findByTestId("notifications-badge")).toHaveTextContent("7");
    expect(screen.getByTestId("notifications-bell")).toHaveAccessibleName(
      "Notifications, 7 unread"
    );
  });

  it("caps the count at 99+", async () => {
    service.unread.mockResolvedValue({ unread: 150 });
    renderBell();
    expect(await screen.findByTestId("notifications-badge")).toHaveTextContent("99+");
  });
});

describe("NotificationsBell inbox", () => {
  it("lists every notification, read and unread, with where it came from and when", async () => {
    renderBell();
    const panel = await openInbox();
    const appRow = await within(panel).findByTestId("notifications-item-n-app");
    expect(appRow).toHaveTextContent("Store Ops");
    expect(appRow).toHaveTextContent("Walk-in cooler list is overdue");
    expect(appRow).toHaveTextContent("3 of 12 checks are still open.");
    expect(appRow).toHaveTextContent("5 minutes ago");
    expect(appRow).toHaveAttribute("data-unread", "true");

    // A platform notice has no app; Oxygen sent it.
    const platformRow = within(panel).getByTestId("notifications-item-n-platform");
    expect(platformRow).toHaveTextContent("Oxygen");
    expect(platformRow).toHaveTextContent("Your workspace is ready");
    expect(platformRow).toHaveAttribute("data-unread", "false");
    expect(service.inbox).toHaveBeenCalledWith({ unreadOnly: false, limit: 50 });
  });

  it("says so when a notification's content could not be read", async () => {
    renderBell();
    const panel = await openInbox();
    const row = await within(panel).findByTestId("notifications-item-n-unreadable");
    expect(row).toHaveTextContent("Couldn't load this notification");
  });

  it("opens an app's notification in this tab, and marks it read", async () => {
    renderBell();
    const panel = await openInbox();
    const row = await within(panel).findByTestId("notifications-item-n-app");
    expect(row).toHaveAttribute("href", "/customer-apps/acme/store-ops/lists/42");
    // A plain anchor, not a router link: the app is a document of its own.
    expect(row).not.toHaveAttribute("target");

    await userEvent.click(row);
    await waitFor(() => expect(service.markRead).toHaveBeenCalledWith("n-app"));
  });

  it("follows a platform notice inside the product, without re-marking it read", async () => {
    renderBell();
    const panel = await openInbox();
    await userEvent.click(await within(panel).findByTestId("notifications-item-n-platform"));
    expect(screen.getByTestId("where")).toHaveTextContent("/acme/workspaces/ws-1/threads");
    await waitFor(() => expect(screen.queryByTestId("notifications-panel")).toBeNull());
    expect(service.markRead).not.toHaveBeenCalled();
  });

  it("marks everything read", async () => {
    renderBell();
    const panel = await openInbox();
    const button = within(panel).getByTestId("notifications-mark-all-read");
    await waitFor(() => expect(button).toBeEnabled());
    await userEvent.click(button);
    await waitFor(() => expect(service.markAllRead).toHaveBeenCalledTimes(1));
    // The bell's "Mark all read" is the whole inbox: no filter.
    expect(service.markAllRead).toHaveBeenCalledWith({});
  });

  it("names the category beside the source, and marks a critical one", async () => {
    service.inbox.mockResolvedValue({ unread: 1, items: [criticalItem, item()] });
    renderBell();
    const panel = await openInbox();
    const row = await within(panel).findByTestId("notifications-item-n-critical");
    expect(row).toHaveTextContent("Store Ops · Food safety");
    expect(row).toHaveAttribute("data-level", "critical");
    expect(row).toHaveTextContent("Critical");
    // From a server with no categories: the source alone, no accent.
    const plain = within(panel).getByTestId("notifications-item-n-app");
    expect(within(plain).getByTestId("notifications-item-source")).toHaveTextContent(/^Store Ops$/);
    expect(plain).not.toHaveAttribute("data-level");
  });

  it("has See all, which opens the notifications page and closes the popover", async () => {
    renderBell();
    const panel = await openInbox();
    const seeAll = within(panel).getByTestId("notifications-see-all");
    expect(seeAll).toHaveAttribute("href", "/notifications");
    await userEvent.click(seeAll);
    expect(screen.getByTestId("where")).toHaveTextContent(/^\/notifications$/);
    await waitFor(() => expect(screen.queryByTestId("notifications-popover")).toBeNull());
  });

  it("has nothing to mark when everything is read", async () => {
    service.inbox.mockResolvedValue({ unread: 0, items: [platformItem] });
    renderBell();
    const panel = await openInbox();
    await within(panel).findByTestId("notifications-item-n-platform");
    expect(within(panel).getByTestId("notifications-mark-all-read")).toBeDisabled();
  });

  it("is a popover under the bell on a wide screen", async () => {
    renderBell();
    await openInbox();
    expect(screen.getByTestId("notifications-popover")).toBeInTheDocument();
    expect(screen.queryByTestId("notifications-sheet")).toBeNull();
  });

  it("shows one line when the inbox is empty", async () => {
    service.inbox.mockResolvedValue({ unread: 0, items: [] });
    renderBell();
    const panel = await openInbox();
    expect(await within(panel).findByTestId("notifications-empty")).toBeInTheDocument();
  });
});

describe("NotificationsBell opened from outside", () => {
  it("opens for ?notifications=open, and drops the param", async () => {
    renderBell("/acme/workspaces/ws-1/home?notifications=open&tab=x");
    expect(await screen.findByTestId("notifications-panel")).toBeInTheDocument();
    await waitFor(() =>
      expect(screen.getByTestId("where")).toHaveTextContent(
        /^\/acme\/workspaces\/ws-1\/home\?tab=x$/
      )
    );
  });

  it("opens when the push worker says its notification was clicked", async () => {
    const worker = new EventTarget();
    Object.defineProperty(navigator, "serviceWorker", { value: worker, configurable: true });
    try {
      renderBell();
      expect(screen.queryByTestId("notifications-panel")).toBeNull();
      act(() => {
        worker.dispatchEvent(
          new MessageEvent("message", { data: { type: "oxy:open-notifications" } })
        );
      });
      expect(await screen.findByTestId("notifications-panel")).toBeInTheDocument();
    } finally {
      Reflect.deleteProperty(navigator, "serviceWorker");
    }
  });

  it("goes where a clicked push says, instead of opening the inbox", async () => {
    const worker = new EventTarget();
    Object.defineProperty(navigator, "serviceWorker", { value: worker, configurable: true });
    try {
      renderBell();
      const href = "/notifications?source=app-1&kind=temp_alarm";
      act(() => {
        worker.dispatchEvent(
          new MessageEvent("message", { data: { type: "oxy:open-notifications", href } })
        );
      });
      await waitFor(() => expect(screen.getByTestId("where")).toHaveTextContent(href));
      expect(screen.queryByTestId("notifications-panel")).toBeNull();
      // Off-site is never followed: the inbox opens instead.
      act(() => {
        worker.dispatchEvent(
          new MessageEvent("message", {
            data: { type: "oxy:open-notifications", href: "//evil.example/x" }
          })
        );
      });
      expect(await screen.findByTestId("notifications-panel")).toBeInTheDocument();
      expect(screen.getByTestId("where")).toHaveTextContent(href);
    } finally {
      Reflect.deleteProperty(navigator, "serviceWorker");
    }
  });
});

describe("NotificationsBell push prompt", () => {
  afterEach(noPushBrowser);
  const PUBLIC_KEY = `B${"A".repeat(86)}`;

  it("offers push on this device between the header and the list", async () => {
    fakePushBrowser();
    service.vapidPublicKey.mockResolvedValue({ configured: true, public_key: PUBLIC_KEY });
    renderBell();
    const panel = await openInbox();
    const prompt = await within(panel).findByTestId("push-prompt");
    expect(prompt).toHaveTextContent("Get notifications on this device");
    expect(within(panel).getByTestId("push-prompt-turn-on")).toBeInTheDocument();
    // After the header, before the list.
    const list = within(panel).getByTestId("notifications-list");
    expect(prompt.compareDocumentPosition(list) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("is in the phone's sheet too", async () => {
    onAPhone();
    fakePushBrowser();
    service.vapidPublicKey.mockResolvedValue({ configured: true, public_key: PUBLIC_KEY });
    renderBell();
    await userEvent.click(screen.getByTestId("notifications-bell"));
    const sheet = await screen.findByTestId("notifications-sheet");
    expect(await within(sheet).findByTestId("push-prompt")).toBeInTheDocument();
  });

  it("is not there where the deployment has no push key", async () => {
    fakePushBrowser();
    renderBell();
    const panel = await openInbox();
    await waitFor(() => expect(service.vapidPublicKey).toHaveBeenCalled());
    await within(panel).findByTestId("notifications-mark-all-read");
    expect(within(panel).queryByTestId("push-prompt")).toBeNull();
  });
});

describe("NotificationsBell on a phone", () => {
  beforeEach(onAPhone);

  it("opens the inbox as a sheet that fills the screen, not a popover", async () => {
    renderBell();
    const panel = await openInbox();
    const sheet = screen.getByTestId("notifications-sheet");
    expect(sheet).toContainElement(panel);
    expect(screen.queryByTestId("notifications-popover")).toBeNull();
    expect(screen.getByRole("dialog", { name: "Notifications" })).toBe(sheet);
    // The sheet takes focus itself, not the close button that comes first while loading.
    expect(sheet).toHaveFocus();
    expect(await within(panel).findByTestId("notifications-item-n-app")).toHaveTextContent(
      "Store Ops"
    );
    expect(within(panel).getByTestId("notifications-item-n-platform")).toHaveTextContent("Oxygen");
  });

  it("opens the sheet for ?notifications=open, and drops the param", async () => {
    renderBell("/acme/workspaces/ws-1/home?notifications=open");
    const sheet = await screen.findByTestId("notifications-sheet");
    expect(within(sheet).getByTestId("notifications-panel")).toBeInTheDocument();
    await waitFor(() =>
      expect(screen.getByTestId("where")).toHaveTextContent(/^\/acme\/workspaces\/ws-1\/home$/)
    );
  });

  it("closes the sheet when a notice is followed", async () => {
    renderBell();
    const panel = await openInbox();
    expect(screen.getByTestId("notifications-sheet")).toBeInTheDocument();
    await userEvent.click(await within(panel).findByTestId("notifications-item-n-platform"));
    expect(screen.getByTestId("where")).toHaveTextContent("/acme/workspaces/ws-1/threads");
    await waitFor(() => expect(screen.queryByTestId("notifications-sheet")).toBeNull());
  });

  it("has See all at the bottom of the sheet, which closes it", async () => {
    renderBell();
    const panel = await openInbox();
    await userEvent.click(within(panel).getByTestId("notifications-see-all"));
    expect(screen.getByTestId("where")).toHaveTextContent(/^\/notifications$/);
    await waitFor(() => expect(screen.queryByTestId("notifications-sheet")).toBeNull());
  });

  it("closes the sheet from its close button", async () => {
    renderBell();
    await openInbox();
    await userEvent.click(screen.getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByTestId("notifications-sheet")).toBeNull());
  });
});
