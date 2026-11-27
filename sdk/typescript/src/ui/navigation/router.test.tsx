// @vitest-environment jsdom
//
// createRouter against jsdom's real session history: a screen is a real URL under the app's
// base path (the main host's /customer-apps/<org>/<app>, an org subdomain's /a/<app>, the root),
// navigate pushes so Back returns to the previous screen, useRoute follows push, replace, Back
// and Forward, and Link navigates in-app only on a plain left click. It binds no keys: ⌘K is
// Ask Oxygen's.

import * as React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { $, click, fire, render } from "../ui.harness";
import { appBase, appPath, createRouter } from "./router";
import { readUrlParam } from "./urlState";

type Route = { name: "home" } | { name: "flags" } | { name: "store"; id: string };

const router = createRouter<Route>({
  parse(path) {
    const [first, second] = path.split("/").filter(Boolean);
    if (first === "flags") return { name: "flags" };
    if (first === "stores" && second) return { name: "store", id: decodeURIComponent(second) };
    return { name: "home" };
  },
  path(route) {
    if (route.name === "flags") return "/flags";
    if (route.name === "store") return `/stores/${encodeURIComponent(route.id)}`;
    return "/";
  },
  title: (route) => `Bookings: ${route.name}`
});

let seen: Route[] = [];
function Screen() {
  const route = router.useRoute();
  seen.push(route);
  return (
    <>
      <output>{route.name === "store" ? `store ${route.id}` : route.name}</output>
      <router.Link to={{ name: "flags" }} data-testid='to-flags'>
        Flags
      </router.Link>
    </>
  );
}

/** A screen that reads its query string, as a report's range does. */
function Report() {
  seen.push(router.useRoute());
  return (
    <>
      <output>{readUrlParam("range") ?? "none"}</output>
      <router.Link to={{ name: "flags" }} search='range=month' data-testid='month'>
        Month
      </router.Link>
    </>
  );
}

/** A screen that only counts its renders. */
function Counted({ onRender }: { onRender: () => void }) {
  router.useRoute();
  onRender();
  return null;
}

const shown = () => $("output")?.textContent;

/** Back or Forward: jsdom fires popstate on a later task. */
async function go(delta: number) {
  await React.act(async () => {
    window.history.go(delta);
    await new Promise((r) => setTimeout(r, 30));
  });
}

async function step(fn: () => void) {
  await React.act(async () => fn());
}

beforeEach(() => {
  seen = [];
  vi.spyOn(window, "scrollTo").mockImplementation(() => undefined);
});

afterEach(() => {
  document.body.innerHTML = "";
  delete window.__OXY_APP__;
  window.history.replaceState(null, "", "/");
  vi.restoreAllMocks();
});

function serveAt(path: string, injected?: { orgSlug: string; slug: string }) {
  if (injected)
    window.__OXY_APP__ = {
      appId: "app-1",
      orgId: "org-1",
      projectId: "p-1",
      branch: "main",
      apiBaseUrl: "",
      ...injected
    };
  window.history.replaceState(null, "", path);
}

describe("the app's base path", () => {
  it("is the main host's prefix, an org subdomain's /a/<app>, or nothing", () => {
    serveAt("/customer-apps/acme/bookings/stores/7", { orgSlug: "acme", slug: "bookings" });
    expect(appBase()).toBe("/customer-apps/acme/bookings");
    expect(appPath()).toBe("/stores/7");
    serveAt("/a/bookings/flags", { orgSlug: "acme", slug: "bookings" });
    expect(appBase()).toBe("/a/bookings");
    expect(appPath()).toBe("/flags");
    serveAt("/flags", { orgSlug: "acme", slug: "bookings" });
    expect(appBase()).toBe("");
    expect(appPath()).toBe("/flags");
  });

  it("keeps the main host's prefix without injection (a local build), and nothing else", () => {
    serveAt("/customer-apps/acme/bookings/flags");
    expect(appBase()).toBe("/customer-apps/acme/bookings");
    serveAt("/a/bookings/flags");
    expect(appBase()).toBe("");
    expect(appPath()).toBe("/a/bookings/flags");
  });
});

describe("createRouter", () => {
  it("reads the route under the base, pushes on navigate, and Back returns to the screen before", async () => {
    serveAt("/a/bookings/", { orgSlug: "acme", slug: "bookings" });
    const before = window.history.length;
    await render(<Screen />);
    expect(shown()).toBe("home");
    await step(() => router.navigate({ name: "store", id: "Alder Creek" }));
    expect(window.location.pathname).toBe("/a/bookings/stores/Alder%20Creek");
    expect(shown()).toBe("store Alder Creek");
    expect(window.history.length).toBe(before + 1);
    expect(window.scrollTo).toHaveBeenCalledWith(0, 0);
    await go(-1);
    expect(window.location.pathname).toBe("/a/bookings/");
    expect(shown()).toBe("home");
    await go(1);
    expect(shown()).toBe("store Alder Creek");
  });

  it("replaces when told to, and opens a destination with its own query", async () => {
    serveAt("/customer-apps/acme/bookings/", { orgSlug: "acme", slug: "bookings" });
    const before = window.history.length;
    await render(<Screen />);
    await step(() =>
      router.navigate({ name: "flags" }, { replace: true, search: "month=2026-07" })
    );
    expect(window.history.length).toBe(before);
    expect(`${window.location.pathname}${window.location.search}`).toBe(
      "/customer-apps/acme/bookings/flags?month=2026-07"
    );
    expect(shown()).toBe("flags");
    expect(document.title).toBe("Bookings: flags");
  });

  it("re-renders when only the query changes: navigate, a Link, Back and Forward", async () => {
    serveAt("/a/bookings/flags?range=week", { orgSlug: "acme", slug: "bookings" });
    await render(<Report />);
    expect(shown()).toBe("week");
    await step(() => router.navigate({ name: "flags" }, { search: "range=day" }));
    expect(window.location.search).toBe("?range=day");
    expect(shown()).toBe("day");
    // The same screen, so the Link replaces the entry (as a re-click does), and still re-renders.
    await click($('[data-testid="month"]'));
    expect(window.location.search).toBe("?range=month");
    expect(shown()).toBe("month");
    await go(-1);
    expect(window.location.search).toBe("?range=week");
    expect(shown()).toBe("week");
    await go(1);
    expect(shown()).toBe("month");
  });

  it("does not re-render when only the hash changes, as before", async () => {
    serveAt("/a/bookings/flags?range=week", { orgSlug: "acme", slug: "bookings" });
    // Its own count: the screens of earlier tests are still mounted (their containers are only
    // detached) and re-render into `seen` when they read a URL other than their last.
    let renders = 0;
    const page = await render(
      <Counted
        onRender={() => {
          renders += 1;
        }}
      />
    );
    expect(renders).toBe(1);
    await step(() => {
      window.history.pushState(null, "", "#notes");
      window.dispatchEvent(new PopStateEvent("popstate"));
    });
    expect(window.location.hash).toBe("#notes");
    expect(renders).toBe(1);
    await page.unmount();
  });

  it("gives real hrefs under the base, so ⌘-click and a copied link open the screen", () => {
    serveAt("/a/bookings/flags", { orgSlug: "acme", slug: "bookings" });
    expect(router.hrefFor({ name: "home" })).toBe("/a/bookings/");
    expect(router.hrefFor({ name: "store", id: "7" }, "?tab=hours")).toBe(
      "/a/bookings/stores/7?tab=hours"
    );
    serveAt("/flags");
    expect(router.hrefFor({ name: "home" })).toBe("/");
  });
});

describe("createRouter's Link", () => {
  it("navigates in-app on a plain left click, with the one focus ring", async () => {
    serveAt("/a/bookings/", { orgSlug: "acme", slug: "bookings" });
    await render(<Screen />);
    const link = $('[data-testid="to-flags"]') as HTMLAnchorElement;
    expect(link.getAttribute("href")).toBe("/a/bookings/flags");
    expect(link.className).toContain("oxy-ui-focus");
    const before = window.history.length;
    await click(link);
    expect(shown()).toBe("flags");
    expect(window.history.length).toBe(before + 1);
  });

  it("replaces rather than stacks when the link is the screen already shown", async () => {
    serveAt("/a/bookings/flags", { orgSlug: "acme", slug: "bookings" });
    await render(<Screen />);
    const before = window.history.length;
    await click($('[data-testid="to-flags"]'));
    expect(window.history.length).toBe(before);
  });

  it("leaves ⌘, Ctrl, Shift and a middle click to the browser", async () => {
    serveAt("/a/bookings/", { orgSlug: "acme", slug: "bookings" });
    await render(<Screen />);
    const link = $('[data-testid="to-flags"]');
    for (const init of [{ metaKey: true }, { ctrlKey: true }, { shiftKey: true }, { button: 1 }]) {
      const ev = new MouseEvent("click", { bubbles: true, cancelable: true, ...init });
      // jsdom would navigate the document on an unprevented click: stop it after the router saw it.
      document.addEventListener("click", (e) => e.preventDefault(), { once: true });
      await fire(link, ev);
      expect(shown(), JSON.stringify(init)).toBe("home");
    }
  });

  it("binds no keys: ⌘K stays Ask Oxygen's", async () => {
    const added = [vi.spyOn(window, "addEventListener"), vi.spyOn(document, "addEventListener")];
    serveAt("/a/bookings/", { orgSlug: "acme", slug: "bookings" });
    await render(<Screen />);
    await step(() => router.navigate({ name: "flags" }));
    const keyed = added.flatMap((spy) =>
      spy.mock.calls.filter(([type]) => /^key/.test(String(type)))
    );
    expect(keyed).toEqual([]);
    const k = new KeyboardEvent("keydown", {
      key: "k",
      metaKey: true,
      bubbles: true,
      cancelable: true
    });
    await fire(document.body, k);
    expect(k.defaultPrevented).toBe(false);
    expect(shown()).toBe("flags");
  });
});
