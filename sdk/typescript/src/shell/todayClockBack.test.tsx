// @vitest-environment jsdom
//
// The phone's Back with the top bar's Today panel open, against jsdom's real
// session history. In the phone layout the open panel holds an entry, as a sheet
// does, so Back closes it and the page stays; on a desktop or a tablet it holds
// none, as before. `here` is where the page stands: on its own entry ("base") the
// next Back leaves the app, on the panel's ("today") it does not.

import * as React from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { servePage } from "../custom-app/notifications.harness";
import { OxyAppProvider } from "../custom-app/react";
import { OxyShell } from "./OxyShell";
import type { ShellContextData } from "./shellContext";

const PAGE = "http://localhost:3000/page";

const SHELL_CONTEXT: ShellContextData = {
  workspace: { id: "ws-1", name: "Default" },
  org: { id: "org-1", slug: "poke-house", name: "Poke House" },
  logo_url: null,
  apps: [],
  links: { home: "https://oxy.test/", threads: "/threads" },
  user: null
};

const WIDGETS = {
  widgets: [
    {
      app: { id: "a", slug: "books", name: "Bookkeeping", org_slug: "poke-house" },
      key: "books/close",
      id: "books/close",
      instance: null,
      title: "Month close",
      params: {},
      size: "regular",
      sizes: ["regular"],
      view: {
        headline: "3 to review",
        detail: null,
        level: "attention",
        progress: null,
        metric: null,
        rows: [],
        buttons: [],
        href: "https://oxy.test/a/books/close",
        badge: null,
        refresh_after: null
      },
      error: null
    }
  ],
  sections: [],
  hidden: [],
  today: "shown"
};

const fetcher = (async (input: RequestInfo | URL) => {
  const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
  if (url.includes("/shell-context")) return new Response(JSON.stringify(SHELL_CONTEXT));
  if (url.startsWith("/api/home/widgets")) return new Response(JSON.stringify(WIDGETS));
  return new Response("{}", { status: 404 });
}) as unknown as typeof fetch;

let page: { unmount(): Promise<void> } | undefined;

const $ = (sel: string) => document.querySelector(sel);
const clock = () => $('[data-testid="topbar-clock"]');
const panel = () => $('[data-testid="topbar-today"]');
const here = () => {
  const state = window.history.state as { page?: string; oxyPhoneSheetDepth?: number } | null;
  if (state?.oxyPhoneSheetDepth) return "today";
  return state?.page;
};

async function wait(ms = 30) {
  await React.act(async () => {
    await new Promise((r) => setTimeout(r, ms));
  });
}

async function click(el: Element | null) {
  if (!el) throw new Error("nothing to click");
  await React.act(async () => {
    el.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
  });
  // Past the exit animation's backstop (240ms), which jsdom never animates.
  await wait(300);
}

async function phoneBack() {
  await React.act(async () => {
    window.history.back();
  });
  await wait(300);
}

let navigations = 0;
(
  globalThis as unknown as {
    jsdom: { virtualConsole: { on(event: string, fn: (e: Error) => void): void } };
  }
).jsdom.virtualConsole.on("jsdomError", (e) => {
  if (e.message.includes("navigation")) navigations += 1;
});

let phone = true;
beforeEach(async () => {
  phone = true;
  navigations = 0;
  vi.stubGlobal("matchMedia", (media: string) => ({
    matches: phone,
    media,
    addEventListener() {},
    removeEventListener() {}
  }));
  servePage();
  window.history.pushState({ page: "base" }, "", PAGE);
  await new Promise((r) => setTimeout(r, 0));
});

afterEach(async () => {
  await page?.unmount();
  page = undefined;
  await wait();
  vi.unstubAllGlobals();
  document.body.innerHTML = "";
});

async function render() {
  // The inbox harness's mount runs on fake timers, and history here is real.
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await React.act(async () => {
    root.render(
      <OxyAppProvider fetcher={fetcher}>
        <OxyShell>app body</OxyShell>
      </OxyAppProvider>
    );
  });
  page = { unmount: async () => React.act(async () => root.unmount()) };
  await wait(100);
}

describe("Today and the phone's Back", () => {
  it("closes the open panel and leaves the page where it was", async () => {
    await render();
    await click(clock());
    expect(panel()).not.toBeNull();
    expect(here()).toBe("today");

    await phoneBack();
    expect(panel()).toBeNull();
    expect(clock()?.getAttribute("aria-expanded")).toBe("false");
    expect(here()).toBe("base");
  });

  it("takes its entry away when the panel closes another way", async () => {
    await render();
    const before = window.history.length;
    await click(clock());
    expect(window.history.length).toBe(before + 1);
    await click(clock());
    await wait(100);
    expect(panel()).toBeNull();
    expect(here()).toBe("base");
  });

  it("cannot trap Back when the app pushes its own entry while the panel is open", async () => {
    await render();
    await click(clock());
    // A router navigation of the app's own, on top of the panel's entry.
    await React.act(async () => window.history.pushState({ page: "app" }, "", `${PAGE}?x=1`));
    await phoneBack();
    // Back off the app's entry lands on the panel's: still open, nothing pushed.
    expect(here()).toBe("today");
    const length = window.history.length;
    await phoneBack();
    expect(panel()).toBeNull();
    expect(here()).toBe("base");
    // Closing pushed nothing, so the next Back leaves rather than loops.
    expect(window.history.length).toBe(length);
  });

  it("goes to a row's page in place of the panel's entry, with no step back under the load", async () => {
    await render();
    await click(clock());
    const before = window.history.length;
    await click($('[data-testid="topbar-today-row"]'));
    expect(panel()).toBeNull();
    expect(navigations).toBe(1);
    // A traversal would cancel the load: the entry stays for the new page to replace.
    await wait(1100);
    expect(window.history.length).toBe(before);
    expect(here()).toBe("today");
  });

  it("on a desktop or a tablet, holds no entry: Back is the browser's, as before", async () => {
    phone = false;
    await render();
    const before = window.history.length;
    await click(clock());
    expect(panel()).not.toBeNull();
    expect(window.history.length).toBe(before);
    expect(here()).toBe("base");
  });
});
