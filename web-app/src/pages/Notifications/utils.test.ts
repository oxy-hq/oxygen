import { describe, expect, it } from "vitest";
import type {
  CategoryNode,
  InboxItem,
  KindNode,
  NotificationCatalog,
  SourceNode
} from "@/types/notifications";
import { collapseThreads, dayLabel, groupByDay, scopeLabel, sourcesInStream } from "./utils";

const item = (id: string, created_at: string, over: Partial<InboxItem> = {}): InboxItem => ({
  id,
  org_id: "org-1",
  kind: "list_overdue",
  created_at,
  read_at: null,
  title: id,
  body: null,
  href: null,
  link: null,
  app: { id: "app-1", slug: "store-ops", name: "Store Ops" },
  thread: null,
  ...over
});

describe("collapseThreads", () => {
  it("keeps the newest of a thread and counts the rest", () => {
    const entries = collapseThreads([
      item("a", "2026-10-14T11:00:00Z", { thread: "t1" }),
      item("b", "2026-10-14T10:00:00Z"),
      item("c", "2026-10-14T09:00:00Z", { thread: "t1" }),
      item("d", "2026-10-14T08:00:00Z", { thread: "t1" })
    ]);
    expect(entries.map((e) => [e.item.id, e.more])).toEqual([
      ["a", 2],
      ["b", 0]
    ]);
  });

  it("does not merge one thread name across two sources", () => {
    const entries = collapseThreads([
      item("a", "2026-10-14T11:00:00Z", { thread: "t1" }),
      item("b", "2026-10-14T10:00:00Z", {
        thread: "t1",
        app: { id: "app-2", slug: "books", name: "Books" }
      })
    ]);
    expect(entries.map((e) => e.more)).toEqual([0, 0]);
  });

  it("leaves items with no thread (or from an older server) alone", () => {
    const old = item("a", "2026-10-14T11:00:00Z");
    delete old.thread;
    expect(collapseThreads([old, item("b", "2026-10-14T10:00:00Z")])).toHaveLength(2);
  });
});

describe("dayLabel", () => {
  const now = new Date(2026, 9, 14, 12);
  it("names today and yesterday, then dates", () => {
    expect(dayLabel(new Date(2026, 9, 14, 1), now)).toBe("Today");
    expect(dayLabel(new Date(2026, 9, 13, 23), now)).toBe("Yesterday");
    expect(dayLabel(new Date(2026, 9, 12, 9), now)).toBe("Mon, Oct 12");
    expect(dayLabel(new Date(2025, 11, 31, 9), now)).toBe("Dec 31, 2025");
  });
});

describe("groupByDay", () => {
  it("groups consecutive entries by local day", () => {
    const now = new Date(2026, 9, 14, 12);
    const groups = groupByDay(
      collapseThreads([
        item("a", new Date(2026, 9, 14, 11).toISOString()),
        item("b", new Date(2026, 9, 14, 9).toISOString()),
        item("c", new Date(2026, 9, 13, 9).toISOString())
      ]),
      now
    );
    expect(groups.map((g) => [g.label, g.entries.map((e) => e.item.id)])).toEqual([
      ["Today", ["a", "b"]],
      ["Yesterday", ["c"]]
    ]);
  });
});

describe("scopeLabel", () => {
  const catalog: NotificationCatalog = {
    reach: { push_devices: 0, email: true },
    sources: [
      {
        source: "app",
        app: { id: "app-1", slug: "store-ops", name: "Store Ops", org_slug: "acme" },
        label: "Store Ops",
        muted: [],
        unread: 0,
        categories: [
          {
            path: "operations",
            label: "Store operations",
            description: null,
            muted: [],
            unread: 0,
            kinds: [],
            categories: [
              {
                path: "operations/food-safety",
                label: "Food safety",
                description: null,
                muted: [],
                unread: 0,
                categories: [],
                kinds: [
                  {
                    kind: "temp_alarm",
                    label: "Temperature alarm",
                    description: null,
                    level: "critical",
                    critical: true,
                    channels: ["push"],
                    muted: [],
                    actions: [],
                    unread: 0
                  }
                ]
              }
            ]
          }
        ]
      }
    ]
  };

  it("names the whole inbox, a source, a category and a kind", () => {
    expect(scopeLabel(catalog, {})).toBe("All notifications");
    expect(scopeLabel(catalog, { source: "app-1" })).toBe("Store Ops");
    expect(scopeLabel(catalog, { source: "app-1", category: "operations/food-safety" })).toBe(
      "Store Ops · Food safety"
    );
    expect(scopeLabel(catalog, { source: "app-1", kind: "temp_alarm" })).toBe(
      "Store Ops · Temperature alarm"
    );
  });

  it("still says something before the catalogue loads", () => {
    expect(scopeLabel(undefined, { source: "oxygen" })).toBe("Oxygen");
  });
});

const kind = (name: string, unread: number, stream?: KindNode["stream"]): KindNode => ({
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

const category = (
  path: string,
  kinds: KindNode[],
  categories: CategoryNode[] = []
): CategoryNode => ({
  path,
  label: path,
  description: null,
  muted: [],
  unread: kinds.reduce((n, k) => n + k.unread, 0) + categories.reduce((n, c) => n + c.unread, 0),
  categories,
  kinds
});

const oxygen = (categories: CategoryNode[]): SourceNode => ({
  source: "oxygen",
  app: null,
  label: "Oxygen",
  muted: [],
  unread: categories.reduce((n, c) => n + c.unread, 0),
  categories
});

const app = (categories: CategoryNode[]): SourceNode => ({
  source: "app",
  app: { id: "app-1", slug: "store-ops", name: "Store Ops", org_slug: "acme" },
  label: "Store Ops",
  muted: [],
  unread: categories.reduce((n, c) => n + c.unread, 0),
  categories
});

describe("sourcesInStream", () => {
  const sources = [
    app([category("operations", [kind("temp_alarm", 3, "activity")])]),
    oxygen([
      category("runs", [kind("oxy.run_failed", 2, "operations")]),
      category("workspaces", [kind("oxy.workspace_ready", 1, "activity")])
    ])
  ];

  it("leaves the tree alone without a stream", () => {
    expect(sourcesInStream(sources, undefined)).toBe(sources);
  });

  it("keeps one stream's categories and drops the sources left with none", () => {
    const operations = sourcesInStream(sources, "operations");
    expect(operations.map((s) => s.label)).toEqual(["Oxygen"]);
    expect(operations[0].categories.map((c) => c.path)).toEqual(["runs"]);
    // The source's own count is re-summed from what survived, so the row does
    // not advertise the other tab's unread.
    expect(operations[0].unread).toBe(2);

    const activity = sourcesInStream(sources, "activity");
    expect(activity.map((s) => s.label)).toEqual(["Store Ops", "Oxygen"]);
    expect(activity[1].categories.map((c) => c.path)).toEqual(["workspaces"]);
    expect(activity[1].unread).toBe(1);
  });

  it("drops a parent whose only kinds were in the other stream", () => {
    const nested = [
      oxygen([category("top", [], [category("top/inner", [kind("k", 1, "operations")])])])
    ];
    expect(sourcesInStream(nested, "activity")).toEqual([]);
    expect(sourcesInStream(nested, "operations")[0].categories[0].path).toBe("top");
  });

  it("shows everything from a server that does not say which stream a kind is in", () => {
    const old = [oxygen([category("runs", [kind("oxy.run_failed", 2)])])];
    expect(sourcesInStream(old, "operations")[0].categories).toHaveLength(1);
    expect(sourcesInStream(old, "activity")[0].categories).toHaveLength(1);
  });
});
