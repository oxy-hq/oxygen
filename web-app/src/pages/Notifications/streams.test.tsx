// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { NotificationsService } from "@/services/api/notifications";
import type {
  CategoryNode,
  InboxItem,
  KindNode,
  NotificationCatalog,
  NotificationStream,
  SourceNode
} from "@/types/notifications";

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

import NotificationsPage from ".";

const service = vi.mocked(NotificationsService);
const STORE_OPS = { id: "app-1", slug: "store-ops", name: "Store Ops" };

const kind = (name: string, stream: NotificationStream, unread: number): KindNode => ({
  kind: name,
  label: name,
  description: null,
  level: "active",
  critical: false,
  channels: ["push"],
  muted: [],
  actions: [],
  unread,
  stream
});

const category = (path: string, kinds: KindNode[]): CategoryNode => ({
  path,
  label: path,
  description: null,
  muted: [],
  unread: kinds.reduce((n, k) => n + k.unread, 0),
  categories: [],
  kinds
});

const source = (over: Partial<SourceNode> & Pick<SourceNode, "categories">): SourceNode => ({
  source: "oxygen",
  app: null,
  label: "Oxygen",
  muted: [],
  unread: over.categories.reduce((n, c) => n + c.unread, 0),
  ...over
});

/** Oxygen with a failed run (Operations), and an app with an alarm (Activity). */
const sources: SourceNode[] = [
  source({ categories: [category("runs", [kind("oxy.run_failed", "operations", 2)])] }),
  source({
    source: "app",
    app: { ...STORE_OPS, org_slug: "acme" },
    label: "Store Ops",
    categories: [category("operations", [kind("temp_alarm", "activity", 4)])]
  })
];

const catalog: NotificationCatalog = {
  reach: { push_devices: 1, email: true },
  sources
};
/** The same tree as someone who runs something sees it. */
const operatorCatalog: NotificationCatalog = { ...catalog, operator: true };

const item: InboxItem = {
  id: "n-1",
  org_id: "org-1",
  kind: "temp_alarm",
  created_at: "2026-10-14T11:55:00Z",
  read_at: null,
  title: "Temperature alarm",
  body: null,
  href: "/customer-apps/acme/store-ops/units/3",
  link: "/units/3",
  app: STORE_OPS
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

const lastInboxCall = () => service.inbox.mock.calls.at(-1)?.[0];
const stopNavigation = (event: MouseEvent) => event.preventDefault();
const user = () => userEvent.setup();

beforeEach(() => {
  document.addEventListener("click", stopNavigation);
  service.catalog.mockResolvedValue(catalog);
  service.inbox.mockResolvedValue({ unread: 1, items: [item], next_before: null });
  service.markAllRead.mockResolvedValue();
  service.unread.mockResolvedValue({
    unread: 6,
    apps: [{ app_id: STORE_OPS.id, unread: 4 }],
    streams: { activity: 4, operations: 2 }
  });
  service.vapidPublicKey.mockResolvedValue({ configured: false, public_key: null });
});

afterEach(() => {
  cleanup();
  document.removeEventListener("click", stopNavigation);
  vi.restoreAllMocks();
  vi.clearAllMocks();
});

describe("Notifications page streams", () => {
  it("shows no stream tabs to someone who runs nothing", async () => {
    renderPage();
    await screen.findByTestId("notifications-page-list");
    expect(screen.queryByTestId("notifications-streams")).toBeNull();
    // Nothing narrows the list either, so an operator notice they already
    // have is still in it.
    expect(lastInboxCall()?.stream).toBeUndefined();
    expect(screen.getByTestId("notifications-tree-oxygen-cat-runs")).toBeTruthy();
  });

  it("opens on Activity for an operator and asks the server for that stream", async () => {
    service.catalog.mockResolvedValue(operatorCatalog);
    renderPage();
    await screen.findByTestId("notifications-streams");
    await waitFor(() => expect(lastInboxCall()?.stream).toBe("activity"));
    // Each tab says how much is unread in it, from the badge's own poll.
    expect(screen.getByTestId("notifications-stream-operations")).toHaveTextContent("2");
  });

  it("switches to Operations, narrows the list, and keeps it in the URL", async () => {
    service.catalog.mockResolvedValue(operatorCatalog);
    renderPage();
    await user().click(await screen.findByTestId("notifications-stream-operations"));
    await waitFor(() => expect(lastInboxCall()?.stream).toBe("operations"));
    expect(screen.getByTestId("where")).toHaveTextContent("stream=operations");
  });

  it("draws only the selected stream's categories in the tree", async () => {
    service.catalog.mockResolvedValue(operatorCatalog);
    renderPage();
    await screen.findByTestId("notifications-tree-store-ops-cat-operations");
    expect(screen.queryByTestId("notifications-tree-oxygen-cat-runs")).toBeNull();
    await user().click(screen.getByTestId("notifications-stream-operations"));
    await waitFor(() =>
      expect(screen.getByTestId("notifications-tree-oxygen-cat-runs")).toBeTruthy()
    );
    expect(screen.queryByTestId("notifications-tree-store-ops-cat-operations")).toBeNull();
  });

  it("marks read only what the open stream holds", async () => {
    service.catalog.mockResolvedValue(operatorCatalog);
    renderPage();
    await screen.findByTestId("notifications-streams");
    const markAll = screen.getByTestId("notifications-page-mark-all-read");
    await waitFor(() => expect(markAll).toBeEnabled());
    await user().click(markAll);
    await waitFor(() =>
      expect(service.markAllRead).toHaveBeenCalledWith(
        expect.objectContaining({ stream: "activity" })
      )
    );
  });

  it("opens a clicked push for a failed run on Operations, not on an empty Activity", async () => {
    // The href a push carries names a kind and never a stream
    // (`web_push_payload::href`), and Activity can never hold `oxy.run_failed`
    // — so defaulting the link to Activity would ask for a list that cannot
    // match, for a link that worked before streams existed.
    service.catalog.mockResolvedValue(operatorCatalog);
    renderPage("/notifications?source=oxygen&kind=oxy.run_failed");
    await screen.findByTestId("notifications-streams");
    await waitFor(() => expect(lastInboxCall()?.kind).toBe("oxy.run_failed"));
    expect(lastInboxCall()?.stream).toBeUndefined();
    // The tab and the tree still follow the kind, so Automations is beside it.
    expect(screen.getByTestId("notifications-stream-operations")).toHaveAttribute(
      "data-state",
      "active"
    );
    expect(screen.getByTestId("notifications-tree-oxygen-cat-runs")).toBeTruthy();
  });

  it("does not narrow a thread link by the tab it was opened from", async () => {
    service.catalog.mockResolvedValue(operatorCatalog);
    renderPage("/notifications?source=app-1&thread=t-cooler&stream=activity");
    await screen.findByTestId("notifications-streams");
    await waitFor(() => expect(lastInboxCall()?.thread).toBe("t-cooler"));
    expect(lastInboxCall()?.stream).toBeUndefined();
    // The URL keeps the tab, so clearing the thread goes back to it.
    expect(screen.getByTestId("where")).toHaveTextContent("stream=activity");
  });

  it("starts the new stream over rather than carrying a category across", async () => {
    // A category of the stream being left would narrow the new list to
    // nothing, so a tab click drops everything but the stream.
    service.catalog.mockResolvedValue(operatorCatalog);
    renderPage("/notifications?source=app-1&category=operations");
    await user().click(await screen.findByTestId("notifications-stream-operations"));
    await waitFor(() => expect(lastInboxCall()?.category).toBeUndefined());
    expect(lastInboxCall()?.source).toBeUndefined();
  });
});
