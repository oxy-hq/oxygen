import { describe, expect, it } from "vitest";
import {
  appBasePath,
  badgeLabel,
  categoryLabel,
  collapseThreads,
  createInboxClient,
  formatAge,
  joinAppPath,
  matchesFilter,
  type NotificationItem,
  notificationTarget,
  serverKnowsCategories,
  serverScopesToApp
} from "./notifications";

const APP = { id: "app-1", slug: "bookings", name: "Bookings" };

function item(over: Partial<NotificationItem> = {}): NotificationItem {
  return {
    id: "n1",
    org_id: "org-1",
    kind: "booking_created",
    created_at: "2026-10-14T12:00:00Z",
    read_at: null,
    title: "New booking",
    body: null,
    href: "/customer-apps/acme/bookings/bookings/1",
    link: "/bookings/1",
    app: APP,
    ...over
  };
}

function recorder(answer: (url: string) => Response | Promise<Response>) {
  const calls: { url: string; method: string }[] = [];
  const fetcher = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    calls.push({ url, method: init?.method ?? "GET" });
    return answer(url);
  }) as typeof fetch;
  return { fetcher, calls };
}

const ok = (body: unknown) =>
  new Response(JSON.stringify(body), {
    status: 200,
    headers: { "content-type": "application/json" }
  });

describe("createInboxClient", () => {
  it("narrows every read and read-all to the app, but not one item's read", async () => {
    const { fetcher, calls } = recorder(() => ok({ unread: 2, items: [] }));
    const client = createInboxClient(fetcher, "", "app-1");
    await client.unread();
    await client.list(20);
    await client.markAllRead();
    await client.markRead("n 1");
    expect(calls).toEqual([
      { method: "GET", url: "/api/notifications/unread?app_id=app-1" },
      { method: "GET", url: "/api/notifications?unread_only=false&limit=20&app_id=app-1" },
      { method: "POST", url: "/api/notifications/read-all?app_id=app-1" },
      { method: "POST", url: "/api/notifications/n%201/read" }
    ]);
  });

  it("drops the filter for the whole inbox and honors apiBaseUrl", async () => {
    const { fetcher, calls } = recorder(() => ok({ unread: 0 }));
    const client = createInboxClient(fetcher, "https://oxy.example.com/", undefined);
    await client.unread();
    await client.markAllRead();
    expect(calls.map((c) => c.url)).toEqual([
      "https://oxy.example.com/api/notifications/unread",
      "https://oxy.example.com/api/notifications/read-all"
    ]);
  });

  it("calls a server without the routes, or no session, unavailable — not an error to show", async () => {
    for (const status of [401, 403, 404]) {
      const { fetcher } = recorder(() => new Response("nope", { status }));
      const r = await createInboxClient(fetcher, "", "app-1").unread();
      expect(r).toMatchObject({ ok: false, unavailable: true });
    }
  });

  it("calls a host that answers every path with a page unavailable", async () => {
    const { fetcher } = recorder(() => new Response("<!doctype html>", { status: 200 }));
    const r = await createInboxClient(fetcher, "", "app-1").list(20);
    expect(r).toMatchObject({ ok: false, unavailable: true });
  });

  it("reports a 500 and a dropped connection as errors, with the inbox still there", async () => {
    const five = recorder(() => new Response("boom", { status: 500 }));
    const r1 = await createInboxClient(five.fetcher, "", "app-1").unread();
    expect(r1).toMatchObject({ ok: false, unavailable: false });
    if (!r1.ok) expect(r1.error.message).toContain("HTTP 500");

    const dropped = (async () => {
      throw new TypeError("Failed to fetch");
    }) as typeof fetch;
    const r2 = await createInboxClient(dropped, "", "app-1").unread();
    expect(r2).toMatchObject({ ok: false, unavailable: false });
  });
});

describe("serverScopesToApp", () => {
  it("knows nothing from an empty page", () => {
    expect(serverScopesToApp([])).toBeNull();
  });
  it("reads a `link` key — even null — as a server that honors app_id", () => {
    expect(serverScopesToApp([item({ link: null })])).toBe(true);
  });
  it("reads a missing `link` as a server that ignored the filter", () => {
    const old = item();
    delete old.link;
    expect(serverScopesToApp([old])).toBe(false);
  });
});

describe("appBasePath", () => {
  it("finds the app under each host scheme", () => {
    expect(appBasePath("/customer-apps/acme/bookings/list", "acme", "bookings")).toBe(
      "/customer-apps/acme/bookings/"
    );
    expect(appBasePath("/customer-apps/acme/bookings", "acme", "bookings")).toBe(
      "/customer-apps/acme/bookings/"
    );
    expect(appBasePath("/a/bookings/list", "acme", "bookings")).toBe("/a/bookings/");
    expect(appBasePath("/list", "acme", "bookings")).toBe("/");
  });
  it("does not match a longer slug that shares the prefix", () => {
    expect(appBasePath("/customer-apps/acme/bookings-old/x", "acme", "bookings")).toBe("/");
    expect(appBasePath("/a/bookings2/", "acme", "bookings")).toBe("/");
  });
  it("joins an app link onto the base", () => {
    expect(joinAppPath("/a/bookings/", "/bookings/1")).toBe("/a/bookings/bookings/1");
    expect(joinAppPath("/", "/bookings/1?tab=2")).toBe("/bookings/1?tab=2");
  });
});

describe("notificationTarget", () => {
  const ctx = { appId: "app-1" };

  it("opens this app's notification in place, by its link", () => {
    expect(notificationTarget(item(), ctx)).toEqual({ kind: "app", link: "/bookings/1" });
  });

  it("has nowhere to go when this app passed no link", () => {
    expect(notificationTarget(item({ link: null }), ctx)).toEqual({ kind: "none" });
  });

  it("reads the link out of href on a server older than `link`", () => {
    const old = item({ href: "/customer-apps/acme/bookings/bookings/1?x=1" });
    delete old.link;
    expect(notificationTarget(old, ctx)).toEqual({ kind: "app", link: "/bookings/1?x=1" });
    const root = item({ href: "/customer-apps/acme/bookings/" });
    delete root.link;
    expect(notificationTarget(root, ctx)).toEqual({ kind: "none" });
  });

  it("refuses a link that would leave the app", () => {
    expect(notificationTarget(item({ link: "//evil.example/x" }), ctx)).toEqual({ kind: "none" });
    expect(notificationTarget(item({ link: "https://evil.example" }), ctx)).toEqual({
      kind: "none"
    });
  });

  it("opens another app's in this tab, as the dock does", () => {
    const other = item({
      app: { id: "app-2", slug: "lists", name: "Lists" },
      href: "/customer-apps/acme/lists/lists/9"
    });
    expect(notificationTarget(other, ctx)).toEqual({
      kind: "away",
      url: "/customer-apps/acme/lists/lists/9",
      window: "_self"
    });
  });

  it("opens a platform notice on Oxygen in this tab, against productBaseUrl when one is set", () => {
    const notice = item({ app: null, link: null, href: "/settings/billing", kind: "oxy.billing" });
    expect(notificationTarget(notice, ctx)).toEqual({
      kind: "away",
      url: "/settings/billing",
      window: "_self"
    });
    expect(
      notificationTarget(notice, { ...ctx, productBaseUrl: "https://app.oxygen-hq.com/" })
    ).toEqual({ kind: "away", url: "https://app.oxygen-hq.com/settings/billing", window: "_self" });
  });

  it("keeps an absolute href as the server sent it", () => {
    const notice = item({ app: null, link: null, href: "https://app.oxygen-hq.com/x" });
    expect(notificationTarget(notice, ctx)).toEqual({
      kind: "away",
      url: "https://app.oxygen-hq.com/x",
      window: null
    });
  });

  it("treats every item as elsewhere when this page has no app id", () => {
    expect(notificationTarget(item(), { appId: undefined })).toMatchObject({ kind: "away" });
  });
});

describe("display", () => {
  it("caps the badge and hides it at zero", () => {
    expect(badgeLabel(0)).toBeNull();
    expect(badgeLabel(7)).toBe("7");
    expect(badgeLabel(120)).toBe("99+");
  });
  it("says how long ago, briefly", () => {
    const now = Date.parse("2026-10-14T12:00:00Z");
    expect(formatAge("2026-10-14T11:59:30Z", now)).toBe("now");
    expect(formatAge("2026-10-14T11:55:00Z", now)).toBe("5m");
    expect(formatAge("2026-10-14T09:00:00Z", now)).toBe("3h");
    expect(formatAge("2026-10-12T12:00:00Z", now)).toBe("2d");
    expect(formatAge("2026-09-01T12:00:00Z", now)).not.toBe("");
    expect(formatAge("not a date", now)).toBe("");
  });
});

describe("the inbox client's filters", () => {
  it("sends category with its source, kind and thread, on the list and on read-all", async () => {
    const { fetcher, calls } = recorder(() => ok({ unread: 0, items: [] }));
    const client = createInboxClient(fetcher, "", "app-1");
    const filter = { source: "app-1", category: "operations", kind: "temp_alarm", thread: "abc" };
    await client.list(5, filter);
    await client.markAllRead(filter);
    expect(calls.map((c) => c.url)).toEqual([
      "/api/notifications?unread_only=false&limit=5&source=app-1&category=operations&kind=temp_alarm&thread=abc&app_id=app-1",
      "/api/notifications/read-all?source=app-1&category=operations&kind=temp_alarm&thread=abc&app_id=app-1"
    ]);
  });

  it("keeps next_before when the server sends it, and leaves it out when it does not", async () => {
    const page = (body: unknown) =>
      createInboxClient(recorder(() => ok(body)).fetcher, "", undefined).list(5);
    expect(await page({ unread: 0, items: [], next_before: "n9" })).toEqual({
      ok: true,
      value: { unread: 0, items: [], next_before: "n9" }
    });
    expect(await page({ unread: 0, items: [] })).toEqual({
      ok: true,
      value: { unread: 0, items: [] }
    });
  });
});

describe("matchesFilter", () => {
  const placed = item({
    kind: "temp_alarm",
    thread: "t1",
    category: { path: "operations/food-safety", labels: ["Store operations", "Food safety"] }
  });

  it("matches a category's subtree, never a sibling that shares its prefix", () => {
    expect(matchesFilter(placed, { category: "operations" })).toBe(true);
    expect(matchesFilter(placed, { category: "operations/food-safety" })).toBe(true);
    expect(matchesFilter(placed, { category: "operations/food" })).toBe(false);
    expect(matchesFilter(placed, { category: "ops" })).toBe(false);
  });

  it("matches kind and thread exactly, and every filter at once", () => {
    expect(matchesFilter(placed, { kind: "temp_alarm", thread: "t1" })).toBe(true);
    expect(matchesFilter(placed, { kind: "list_overdue" })).toBe(false);
    expect(matchesFilter(placed, { thread: "t2" })).toBe(false);
    expect(matchesFilter(placed, {})).toBe(true);
  });

  it("reads an item from a server older than categories as general, with no thread", () => {
    expect(matchesFilter(item(), { category: "general" })).toBe(true);
    expect(matchesFilter(item(), { category: "operations" })).toBe(false);
    expect(matchesFilter(item(), { thread: "t1" })).toBe(false);
  });
});

describe("serverKnowsCategories", () => {
  it("is true when every item carries a category, false when one does not, null on none", () => {
    const placed = item({ category: { path: "general", labels: ["General"] } });
    expect(serverKnowsCategories([placed])).toBe(true);
    expect(serverKnowsCategories([placed, item()])).toBe(false);
    expect(serverKnowsCategories([])).toBeNull();
  });
});

describe("collapseThreads", () => {
  const n = (id: string, thread: string | null | undefined) => item({ id, thread });

  it("folds each run of one thread into its newest, keeping order", () => {
    const groups = collapseThreads([
      n("a", "t1"),
      n("b", "t1"),
      n("c", null),
      n("d", "t1"),
      n("e", "t2"),
      n("f", "t2"),
      n("g", "t2")
    ]);
    expect(groups.map((g) => [g.item.id, g.more.map((i) => i.id)])).toEqual([
      ["a", ["b"]],
      ["c", []],
      ["d", []],
      ["e", ["f", "g"]]
    ]);
  });

  it("never folds items without a thread, nor an older server's", () => {
    const groups = collapseThreads([
      n("a", null),
      n("b", null),
      n("c", undefined),
      n("d", undefined)
    ]);
    expect(groups.map((g) => g.more.length)).toEqual([0, 0, 0, 0]);
  });
});

describe("categoryLabel", () => {
  it("names the leaf, with the whole path beside it", () => {
    const placed = item({
      category: { path: "operations/food-safety", labels: ["Store operations", "Food safety"] }
    });
    expect(categoryLabel(placed)).toEqual({
      leaf: "Food safety",
      full: "Store operations › Food safety"
    });
  });

  it("says nothing for general, or for an older server's item", () => {
    expect(categoryLabel(item({ category: { path: "general", labels: ["General"] } }))).toBeNull();
    expect(categoryLabel(item())).toBeNull();
  });
});
