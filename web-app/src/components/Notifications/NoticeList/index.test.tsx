// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { InboxItem } from "@/types/notifications";
import { NoticeList } from ".";

/**
 * The notifications list, over the real inbox: Unread first, a row read
 * there stays in place until the view is chosen again, "Mark all read" empties
 * it, the row's corner link follows the notification and reads it, and the
 * list reads the inbox only while its panel is open.
 */

const item = (id: string, extra: Partial<InboxItem> = {}): InboxItem => ({
  id,
  org_id: "org-acme",
  kind: "app.notice",
  created_at: new Date().toISOString(),
  read_at: null,
  title: `Title ${id}`,
  body: `Body ${id}`,
  href: `/customer-apps/acme/store-ops/notice/${id}`,
  link: null,
  app: { id: "app-1", slug: "store-ops", name: "Store Ops" },
  ...extra
});

let items: InboxItem[] = [];
const inboxEnabled = vi.fn();
const markOne = vi.fn();
const markAll = vi.fn();
vi.mock("@/hooks/api/notifications", () => ({
  useNotificationInbox: (enabled: boolean) => {
    inboxEnabled(enabled);
    return { data: { unread: items.filter((i) => !i.read_at).length, items }, isPending: false };
  },
  useMarkNotificationRead: () => ({ mutate: markOne }),
  useMarkAllNotificationsRead: () => ({ mutate: markAll, isPending: false })
}));
vi.mock("@/hooks/api/customApps/useOrgCustomApps", () => ({
  default: () => ({ apps: [], isPending: false })
}));
vi.mock("@/stores/useCurrentWorkspace", () => ({ default: () => ({ workspace: { id: "ws-1" } }) }));
vi.mock("@/components/Notifications/PushPrompt", () => ({ PushPrompt: () => null }));

const renderList = (open = true) =>
  render(
    <MemoryRouter>
      <NoticeList open={open} />
    </MemoryRouter>
  );
const listed = () =>
  screen.queryAllByTestId(/^notifications-item-n/).map((el) => el.dataset.testid);

beforeEach(() => {
  items = [
    item("n1"),
    item("n2", { app: null, kind: "oxy.digest", href: null, title: "From Oxygen" }),
    item("n3", { read_at: "2026-11-01T00:00:00Z" })
  ];
});
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("the notifications list", () => {
  it("shows the unread ones first, and every one under All", () => {
    renderList();
    expect(listed()).toEqual(["notifications-item-n1", "notifications-item-n2"]);
    fireEvent.click(screen.getByTestId("notifications-filter-all"));
    expect(listed()).toEqual([
      "notifications-item-n1",
      "notifications-item-n2",
      "notifications-item-n3"
    ]);
  });

  it("names Oxygen as the sender of a platform notice, which has no link to follow", () => {
    renderList();
    const row = screen.getByTestId("notifications-item-n2");
    expect(within(row).getByText("Oxygen")).toBeTruthy();
    expect(within(row).queryByLabelText(/^Open in/)).toBeNull();
  });

  it("reads a row pressed, keeping it in the Unread view until the view is chosen again", () => {
    const { rerender } = renderList();
    fireEvent.click(screen.getByRole("button", { name: "Mark read: Title n1" }));
    expect(markOne).toHaveBeenCalledWith(expect.objectContaining({ id: "n1" }));
    // The server answers: n1 is read now.
    items = items.map((i) => (i.id === "n1" ? { ...i, read_at: new Date().toISOString() } : i));
    rerender(
      <MemoryRouter>
        <NoticeList open />
      </MemoryRouter>
    );
    expect(listed()).toContain("notifications-item-n1");
    expect(screen.getByTestId("notifications-item-n1").dataset.unread).toBe("false");
    fireEvent.click(screen.getByTestId("notifications-filter-all"));
    fireEvent.click(screen.getByTestId("notifications-filter-unread"));
    expect(listed()).toEqual(["notifications-item-n2"]);
  });

  it("does not mark a read row read again", () => {
    renderList();
    fireEvent.click(screen.getByTestId("notifications-filter-all"));
    fireEvent.click(screen.getByRole("button", { name: "Title n3" }));
    expect(markOne).not.toHaveBeenCalled();
  });

  it("marks everything read and empties the Unread view", () => {
    renderList();
    fireEvent.click(screen.getByTestId("notifications-mark-all-read"));
    expect(markAll).toHaveBeenCalledWith({});
    items = items.map((i) => ({ ...i, read_at: i.read_at ?? new Date().toISOString() }));
    cleanup();
    renderList();
    expect(listed()).toEqual([]);
    expect(screen.getByTestId("notifications-empty").textContent).toBe("No unread notifications.");
  });

  it("follows a row's link into its app, a real address, and reads it", () => {
    renderList();
    const open = screen.getByTestId("notifications-item-open-n1");
    expect(open.getAttribute("href")).toBe("/customer-apps/acme/store-ops/notice/n1");
    expect(open.getAttribute("aria-label")).toBe("Open in Store Ops");
    open.addEventListener("click", (e) => e.preventDefault());
    fireEvent.click(open);
    expect(markOne).toHaveBeenCalledWith(expect.objectContaining({ id: "n1" }));
  });

  it("reads the inbox only while its panel is open", () => {
    renderList(false);
    expect(inboxEnabled).toHaveBeenLastCalledWith(false);
    cleanup();
    renderList(true);
    expect(inboxEnabled).toHaveBeenLastCalledWith(true);
  });

  it("says when a notification's text could not be read", () => {
    items = [item("n9", { title: null, body: null })];
    renderList();
    expect(
      within(screen.getByTestId("notifications-item-n9")).getByText(
        "Couldn't load this notification"
      )
    ).toBeTruthy();
  });
});
