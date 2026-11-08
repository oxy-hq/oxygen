// @vitest-environment jsdom
//
// The clock in `<OxyShell>`'s top bar opens Today: the person's widget cards
// from every app, one `GET /api/home/widgets?org=<slug>` the first time it is
// opened. Mounted for real against a fake server; the panel's look lives in
// shell.css and is only read as text here.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import * as React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { click, mount, type Page, servePage, settle } from "../custom-app/notifications.harness";
import { OxyShell } from "./OxyShell";
import type { ShellContextData } from "./shellContext";
import { safeUrl, todayRows } from "./todayModel";

const SHELL_CONTEXT: ShellContextData = {
  workspace: { id: "ws-1", name: "Default" },
  org: { id: "org-1", slug: "poke house", name: "Poke House" },
  logo_url: null,
  apps: [],
  links: { home: "/", threads: "/threads" },
  user: null
};

const app = (slug: string, name: string, extra: object = {}) => ({
  id: `id-${slug}`,
  slug,
  name,
  org_slug: "poke-house",
  url: `https://oxy.test/a/${slug}/`,
  ...extra
});

const card = (key: string, appObj: object, title: string, view: object | null, error?: string) => ({
  app: appObj,
  key,
  id: key,
  instance: null,
  title,
  params: {},
  size: "regular",
  sizes: ["regular"],
  view,
  error: error ?? null
});

const view = (headline: string, extra: object = {}) => ({
  headline,
  detail: null,
  level: "info",
  progress: null,
  metric: null,
  rows: [],
  buttons: [],
  href: null,
  badge: null,
  refresh_after: null,
  ...extra
});

// Server order is deliberately not display order.
const PAYLOAD = {
  widgets: [
    card("ops/failing", app("ops", "Store Ops"), "Store checks", null, "boom"),
    // Calm with progress: its count stays hidden ("0/17" beside "on time" is noise).
    card(
      "books/calm",
      app("books", "Bookkeeping"),
      "Books",
      view("All reconciled", { progress: { done: 0, total: 17 } })
    ),
    card(
      "books/close",
      app("books", "Bookkeeping", { icon_url: "/icons/books.png" }),
      "Month close",
      view("3 to review", {
        level: "attention",
        progress: { done: 3, total: 5 },
        href: "https://oxy.test/a/books/close"
      })
    ),
    card(
      "ops/asks",
      app("ops", "Store Ops"),
      "Deliveries",
      view("1 late", { buttons: [{ action: "ack", label: "Ack" }] })
    )
  ],
  sections: [],
  hidden: [],
  today: "shown"
};

let page: Page | undefined;
let calls: string[];

const urlOf = (input: RequestInfo | URL): string =>
  typeof input === "string" ? input : input instanceof URL ? input.href : input.url;

type Answer = () => Response | Promise<Response>;
const json =
  (body: unknown, status = 200): Answer =>
  () =>
    new Response(JSON.stringify(body), { status });

async function render(widgets: Answer = json(PAYLOAD)): Promise<Page> {
  calls = [];
  const fetcher = (async (input: RequestInfo | URL) => {
    const url = urlOf(input);
    calls.push(url);
    if (url.includes("/shell-context")) return new Response(JSON.stringify(SHELL_CONTEXT));
    if (url.startsWith("/api/home/widgets")) return widgets();
    return new Response("{}", { status: 404 });
  }) as unknown as typeof fetch;
  page = await mount(<OxyShell>app body</OxyShell>, fetcher);
  await settle();
  return page;
}

const $ = (sel: string) => document.querySelector(sel);
const clock = () => $('[data-testid="topbar-clock"]') as HTMLButtonElement;
const panel = () => $('[data-testid="topbar-today"]');
const widgetCalls = () => calls.filter((u) => u.startsWith("/api/home/widgets"));
const rowTexts = () =>
  [...document.querySelectorAll('[data-testid="topbar-today-row"]')].map(
    (r) => r.querySelector(".oxy-today__row-title")?.textContent
  );

async function key(name: string) {
  await React.act(async () => {
    document.dispatchEvent(new KeyboardEvent("keydown", { key: name, bubbles: true }));
  });
  await settle(300);
}

beforeEach(() => {
  vi.useFakeTimers();
  servePage();
});

afterEach(async () => {
  await page?.unmount();
  page = undefined;
  vi.restoreAllMocks();
  vi.useRealTimers();
});

describe("Today from the top bar clock", () => {
  it("is a button that reads closed, and fetches nothing until opened", async () => {
    await render();
    expect(clock().tagName).toBe("BUTTON");
    expect(clock().getAttribute("aria-haspopup")).toBe("dialog");
    expect(clock().getAttribute("aria-expanded")).toBe("false");
    expect(panel()).toBeNull();
    expect(widgetCalls()).toEqual([]);
  });

  it("opens from the clock and asks once, for the org, credentialed by the app's fetcher", async () => {
    await render();
    await click(clock());
    expect(panel()?.getAttribute("aria-label")).toBe("Today");
    expect(clock().getAttribute("aria-expanded")).toBe("true");
    expect(panel()?.querySelector(".oxy-today__title")?.textContent).toBe("Today");
    expect(widgetCalls()).toEqual(["/api/home/widgets?org=poke%20house"]);

    // Closed and opened again within the minute: nothing more is asked.
    await click(clock());
    await settle(300);
    expect(panel()).toBeNull();
    await click(clock());
    expect(widgetCalls()).toHaveLength(1);
  });

  it("asks again when the last answer is a minute old", async () => {
    await render();
    await click(clock());
    await click(clock());
    await settle(300);
    await settle(61_000);
    await click(clock());
    expect(widgetCalls()).toHaveLength(2);
  });

  it("orders needs-attention first, then calm, then failed last", async () => {
    await render();
    await click(clock());
    expect(rowTexts()).toEqual(["Month close", "Deliveries", "Books", "Store checks"]);
    const groups = [...document.querySelectorAll('[data-testid="topbar-today-row"]')].map((r) =>
      r.getAttribute("data-group")
    );
    expect(groups).toEqual(["attention", "attention", "calm", "failed"]);
    const all = [...document.querySelectorAll('[data-testid="topbar-today-row"]')];
    const failed = all[all.length - 1];
    expect(failed?.querySelector(".oxy-today__row-headline")?.textContent).toBe("Couldn't load");
  });

  it("links each row to the view's link, else the app, in this tab", async () => {
    await render();
    await click(clock());
    const rows = [...document.querySelectorAll('a[data-testid="topbar-today-row"]')];
    expect(rows.map((r) => r.getAttribute("href"))).toEqual([
      "https://oxy.test/a/books/close",
      "https://oxy.test/a/ops/",
      "https://oxy.test/a/books/",
      "https://oxy.test/a/ops/"
    ]);
    for (const r of rows) expect(r.getAttribute("target")).toBeNull();
  });

  it("shows the icon, else initials, and the count only when there is progress", async () => {
    await render();
    await click(clock());
    const first = $('[data-testid="topbar-today-row"]');
    expect(first?.querySelector("img")?.getAttribute("src")).toBe("/icons/books.png");
    const count = first?.querySelector(".oxy-today__count");
    expect(count?.textContent).toBe("3/5");
    expect(document.querySelectorAll(".oxy-today__count")).toHaveLength(1);
    const initials = [...document.querySelectorAll(".oxy-today__mark")].map((m) => m.textContent);
    expect(initials).toEqual(["", "SO", "B", "SO"]);
  });

  it("footer opens the org home like the rail's HQ", async () => {
    await render();
    await click(clock());
    expect($('[data-testid="topbar-today-home"]')?.getAttribute("href")).toBe("/");
    expect($('[data-testid="topbar-today-home"]')?.textContent).toBe("Open Oxygen home");
  });

  it("says so when there is nothing", async () => {
    await render(json({ widgets: [] }));
    await click(clock());
    expect($('[data-testid="topbar-today-empty"]')?.textContent).toBe("Nothing for you today.");
  });

  it("says it could not load on a refusal, a dropped request, or a payload that is not a list", async () => {
    for (const answer of [
      json({ error: "no" }, 500),
      () => Promise.reject(new TypeError("offline")),
      json("<html>"),
      json({ widgets: "nope" }),
      json(null)
    ] as Answer[]) {
      await render(answer);
      await click(clock());
      expect($('[data-testid="topbar-today-error"]')?.textContent).toBe("Couldn't load Today.");
      expect($('[data-testid="topbar-today-home"]')).not.toBeNull();
      await page?.unmount();
    }
    page = undefined;
  });

  it("shows loading while the request is out", async () => {
    let release: (r: Response) => void = () => {};
    await render(() => new Promise<Response>((r) => (release = r)));
    await click(clock());
    expect($('[data-testid="topbar-today-loading"]')).not.toBeNull();
    await React.act(async () => release(new Response(JSON.stringify(PAYLOAD))));
    await settle();
    expect($('[data-testid="topbar-today-loading"]')).toBeNull();
    expect(rowTexts()).toHaveLength(4);
  });

  it("closes on Escape and gives focus back to the clock", async () => {
    await render();
    await click(clock());
    expect(document.activeElement).toBe(panel());
    await key("Escape");
    expect(panel()).toBeNull();
    expect(clock().getAttribute("aria-expanded")).toBe("false");
    expect(document.activeElement).toBe(clock());
  });

  it("closes on a press outside, and not on one inside", async () => {
    await render();
    await click(clock());
    await React.act(async () => {
      panel()?.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    });
    await settle(300);
    expect(panel()).not.toBeNull();
    await React.act(async () => {
      document.body.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    });
    await settle(300);
    expect(panel()).toBeNull();
  });

  it("marks the exit before it unmounts, so it can play", async () => {
    await render();
    await click(clock());
    await React.act(async () => {
      document.body.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    });
    expect(panel()?.getAttribute("data-state")).toBe("closed");
    await settle(300);
    expect(panel()).toBeNull();
  });

  it("keeps the plain clock when the shell has no org to ask for", async () => {
    calls = [];
    const fetcher = (async (input: RequestInfo | URL) => {
      calls.push(urlOf(input));
      return new Response("{}", { status: 503 });
    }) as unknown as typeof fetch;
    page = await mount(<OxyShell topBarExtra={<i />}>app body</OxyShell>, fetcher);
    await settle();
    // Still a clock, not a button that would open onto nothing.
    expect(clock().tagName).toBe("SPAN");
    expect(widgetCalls()).toEqual([]);
  });

  it("keeps the plain clock when the app asks for it, with an org to ask for", async () => {
    calls = [];
    const fetcher = (async (input: RequestInfo | URL) => {
      const url = urlOf(input);
      calls.push(url);
      if (url.includes("/shell-context")) return new Response(JSON.stringify(SHELL_CONTEXT));
      return new Response(JSON.stringify(PAYLOAD));
    }) as unknown as typeof fetch;
    page = await mount(<OxyShell plainClock>app body</OxyShell>, fetcher);
    await settle();
    // The shell loaded (the org is there), and the clock is still just a time.
    expect(calls.some((u) => u.includes("/shell-context"))).toBe(true);
    expect(clock().tagName).toBe("SPAN");
    expect(clock().className).toBe("oxy-shell-scope oxy-clock");
    expect(clock().getAttribute("aria-haspopup")).toBeNull();
    expect($(".oxy-clock__icon")).toBeNull();
    await click(clock());
    await settle(300);
    expect(panel()).toBeNull();
    expect(widgetCalls()).toEqual([]);
  });
});

describe("todayRows", () => {
  it("skips entries it cannot read and never throws", () => {
    const rows = todayRows({
      widgets: [
        null,
        7,
        "x",
        {},
        { title: "No view" },
        { app: null, title: "T", view: { headline: "ok", progress: { done: "a", total: 0 } } },
        { title: 5, app: { name: "Named" }, view: { headline: 9 } }
      ]
    });
    expect(rows?.map((r) => [r.title, r.group, r.headline, r.progress])).toEqual([
      ["T", "calm", "ok", null],
      ["No view", "failed", "Couldn't load", null],
      ["Named", "failed", "Couldn't load", null]
    ]);
    for (const bad of [undefined, null, 1, "s", [], {}, { widgets: {} }]) {
      expect(todayRows(bad)).toBeNull();
    }
  });

  it("clamps progress and reads the old names (warning, link, actions) too", () => {
    const [row] = todayRows({
      widgets: [
        {
          title: "T",
          app: { name: "A" },
          view: { headline: "h", level: "warning", link: "/x", progress: { done: 9, total: 3 } }
        }
      ]
    }) ?? [null];
    expect(row?.group).toBe("attention");
    expect(row?.href).toBe("/x");
    expect(row?.progress?.fraction).toBe(1);
  });

  it("only lets http(s) and app-relative links into an href", () => {
    expect(safeUrl("https://a.test/x")).toBe("https://a.test/x");
    expect(safeUrl("/a/b")).toBe("/a/b");
    for (const bad of [
      "javascript:alert(1)",
      "//evil.test",
      "/\\evil.test",
      "data:text/html,x",
      " ",
      5,
      null
    ]) {
      expect(safeUrl(bad)).toBeNull();
    }
    const [row] = todayRows({
      widgets: [
        {
          title: "T",
          app: { name: "A", url: "javascript:1" },
          view: { headline: "h", href: "//x.test" }
        }
      ]
    }) ?? [null];
    expect(row?.href).toBeNull();
  });
});

describe("Today styles", () => {
  const here = dirname(fileURLToPath(import.meta.url));
  const css = readFileSync(join(here, "shell.css"), "utf8");

  it("keeps Today on the motion tokens and drops travel under reduced motion", () => {
    const today = css.slice(css.indexOf(".oxy-today {"));
    expect(today).toMatch(
      /animation: oxy-today-in var\(--oxy-duration-base\) var\(--oxy-ease-out\)/
    );
    expect(today).toMatch(
      /animation: oxy-today-out var\(--oxy-duration-fast\) var\(--oxy-ease-in\)/
    );
    expect(today).toMatch(/translateY\(-4px\) scale\(0\.97\)/);
    expect(today).toMatch(
      /prefers-reduced-motion: reduce\) \{\s*\.oxy-today \{\s*animation-name: oxy-today-fade-in/
    );
  });
});
