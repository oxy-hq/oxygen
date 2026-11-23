// @vitest-environment jsdom
//
// The rail opens and closes from its toggle (2.33.0), as ChatGPT's and Gemini's sidebars do and
// as the web-app's rail does: closed, 48px of marks named in hints, the workspace mark the toggle;
// open, 14rem with names and a close toggle, in the frame's flow so the app narrows beside it.
// Hovering opens nothing. The state is remembered per browser under the web-app's key. Mounted
// for real inside `<OxyShell>` against a fake shell-context; the widths live in shell.css, checked
// as text like the other shell tests.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import * as React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { click, mount, type Page, servePage, settle } from "../custom-app/notifications.harness";
import { NAV_SHEET_QUERY } from "./NavSheet";
import { OxyShell } from "./OxyShell";
import { RAIL_EXPANDED_KEY, ShellRail } from "./ShellRail";
import type { ShellContextData } from "./shellContext";

const SHELL_CONTEXT: ShellContextData = {
  workspace: { id: "ws-1", name: "Default" },
  org: { id: "org-1", slug: "acme", name: "Poke House" },
  logo_url: null,
  apps: [
    { id: "app-1", name: "Bookings", slug: "bookings", url: "/customer-apps/acme/bookings/" },
    { id: "app-2", name: "Command Center", slug: "command-center", url: "/customer-apps/acme/cc/" }
  ],
  links: { home: "/", threads: "/threads" },
  user: null
};

const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "shell.css"), "utf8");

let page: Page | undefined;

async function render(ui: React.ReactNode = <OxyShell>app body</OxyShell>): Promise<Page> {
  const fetcher = (async () =>
    new Response(JSON.stringify(SHELL_CONTEXT), { status: 200 })) as unknown as typeof fetch;
  page = await mount(ui, fetcher);
  await settle();
  return page;
}

const $ = (sel: string) => document.querySelector(sel);
const rail = () => $('[data-testid="shell-rail"]') as HTMLElement;
const toggle = () => $('[data-testid="shell-rail-toggle"]') as HTMLButtonElement;
const title = () => $('[data-testid="shell-rail-title"]');

/** `matchMedia` answering `narrow` for the nav sheet's query (and no to every other), with
 *  `set` to cross the breakpoint as a rotation or a resize does. */
function stubNavSheetQuery(narrow: boolean) {
  const state = { narrow };
  const listeners = new Set<() => void>();
  vi.stubGlobal("matchMedia", (q: string) => ({
    get matches() {
      return q === NAV_SHEET_QUERY && state.narrow;
    },
    media: q,
    addEventListener: (_: string, fn: () => void) => listeners.add(fn),
    removeEventListener: (_: string, fn: () => void) => listeners.delete(fn)
  }));
  return {
    async set(next: boolean) {
      state.narrow = next;
      await React.act(async () => {
        for (const fn of listeners) fn();
      });
    }
  };
}

beforeEach(() => {
  vi.useFakeTimers();
  servePage();
});

afterEach(async () => {
  await page?.unmount();
  page = undefined;
  localStorage.clear();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe("ShellRail opening and closing", () => {
  it("starts closed, with the workspace mark as its toggle", async () => {
    await render();
    expect(rail().hasAttribute("data-expanded")).toBe(false);
    expect(rail().classList.contains("oxy-rail--expanded")).toBe(false);
    expect(toggle().getAttribute("aria-label")).toBe("Expand sidebar");
    expect(toggle().getAttribute("aria-expanded")).toBe("false");
    expect(toggle().querySelector('[data-testid="rail-workspace"]')).not.toBeNull();
    expect(title()).toBeNull();
    // Closed, each entry is still named for a screen reader.
    expect($('[data-testid="rail-app-bookings"]')?.getAttribute("aria-label")).toBe("Bookings");
  });

  // The complaint this answers: a rail that opens on hover reads as the rail hiding the page.
  it("does not open on hover", async () => {
    await render();
    for (const type of ["pointerover", "pointerenter", "mouseover", "mouseenter"]) {
      rail().dispatchEvent(new MouseEvent(type, { bubbles: true }));
    }
    await settle();
    expect(rail().hasAttribute("data-expanded")).toBe(false);
  });

  it("opens and closes from its toggle, naming the org and each entry while open", async () => {
    await render();
    await click(toggle());
    expect(rail().hasAttribute("data-expanded")).toBe(true);
    expect(rail().classList.contains("oxy-rail--expanded")).toBe(true);
    expect(toggle().getAttribute("aria-label")).toBe("Collapse sidebar");
    expect(toggle().getAttribute("aria-expanded")).toBe("true");
    expect(title()?.textContent).toBe("Poke House");
    expect($('[data-testid="rail-workspace"]')).not.toBeNull();
    const labels = [...document.querySelectorAll(".oxy-rail__item-label")].map(
      (l) => l.textContent
    );
    expect(labels).toEqual(["Home", "Bookings", "Command Center"]);

    await click(toggle());
    expect(rail().hasAttribute("data-expanded")).toBe(false);
    expect(title()).toBeNull();
  });

  // The closed and open heads draw the toggle in different places; a remount there would drop
  // keyboard focus to the page on every Enter.
  it("keeps keyboard focus on the toggle as it opens and closes", async () => {
    await render();
    toggle().focus();
    await click(toggle());
    expect(document.activeElement).toBe(toggle());
    await click(toggle());
    expect(document.activeElement).toBe(toggle());
  });

  it("remembers how it was left, under the web-app's key", async () => {
    await render();
    await click(toggle());
    expect(localStorage.getItem(RAIL_EXPANDED_KEY)).toBe("true");
    expect(RAIL_EXPANDED_KEY).toBe("oxy.shell-rail.expanded");
    await page?.unmount();

    await render();
    expect(rail().hasAttribute("data-expanded")).toBe(true);
  });

  it("starts closed when storage is blocked, and still opens", async () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    await render();
    expect(rail().hasAttribute("data-expanded")).toBe(false);
    await click(toggle());
    expect(rail().hasAttribute("data-expanded")).toBe(true);
  });

  // An app that draws ShellRail itself, with no `top`, still gets the head and the toggle.
  it("gives a rail with no top mark a toggle of its own", async () => {
    await render(<ShellRail groups={[[{ key: "hq", label: "HQ", letter: "H" }]]} />);
    expect(toggle().getAttribute("aria-label")).toBe("Expand sidebar");
    await click(toggle());
    expect(rail().hasAttribute("data-expanded")).toBe(true);
  });
});

// Below 768px and on a phone on its side the shell draws the nav sheet instead of the rail, but an
// app that hides the top bar keeps the rail there (the sheet's button lives in the top bar), and so
// does a ShellRail an app draws itself. Open, 14rem would leave a 375px phone about 150px; so there
// the rail is the 2.32 rail: closed, no toggle, the mark plain branding.
describe("ShellRail where the nav sheet stands in for it", () => {
  it("draws a hideTopBar app's rail on a phone as 2.32 did, and leaves the desk's choice alone", async () => {
    localStorage.setItem(RAIL_EXPANDED_KEY, "true");
    const media = stubNavSheetQuery(true);
    await render(<OxyShell hideTopBar>app body</OxyShell>);
    expect(rail()).not.toBeNull();
    expect(rail().hasAttribute("data-expanded")).toBe(false);
    expect(toggle()).toBeNull();
    expect($(".oxy-rail__top")?.querySelector('[data-testid="rail-workspace"]')).not.toBeNull();
    expect($(".oxy-rail__top")?.querySelector("button")).toBeNull();
    expect(title()).toBeNull();
    expect(localStorage.getItem(RAIL_EXPANDED_KEY)).toBe("true");

    // Back on a desk-wide window, the rail opens as it was left there.
    await media.set(false);
    expect(rail().hasAttribute("data-expanded")).toBe(true);
    expect(toggle()?.getAttribute("aria-label")).toBe("Collapse sidebar");
  });

  it("gives a ShellRail an app draws itself no toggle there either", async () => {
    stubNavSheetQuery(true);
    await render(
      <ShellRail
        top={<span data-testid='mark'>P</span>}
        groups={[[{ key: "hq", label: "HQ", letter: "H" }]]}
      />
    );
    expect(toggle()).toBeNull();
    expect($('[data-testid="mark"]')?.closest("button")).toBeNull();
    expect(rail().hasAttribute("data-expanded")).toBe(false);
  });
});

describe("shell.css: the open rail takes its width in the frame's flow", () => {
  const block = (selector: string) => {
    const esc = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    return new RegExp(`(?:^|[}\\s])${esc}\\s*\\{([^}]*)\\}`).exec(css)?.[1] ?? "";
  };

  it("widens the rail itself, which is a flex item beside the content column", () => {
    expect(block(".oxy-rail.oxy-rail--expanded")).toMatch(/width: 14rem;/);
    // Nothing positions the rail out of the flow, so the column beside it gives way.
    for (const [, body] of css.matchAll(/\.oxy-rail(?:\.oxy-rail--expanded)?\s*\{([^}]*)\}/g)) {
      expect(body).not.toMatch(/position:\s*(absolute|fixed)/);
    }
    expect(block(".oxy-shell__content-col")).toMatch(/min-width: 0;[\s\S]*flex: 1;/);
  });

  it("eases the width, and not under reduced motion", () => {
    expect(css).toMatch(
      /\.oxy-rail \{\s*transition: width var\(--oxy-duration-fast\) var\(--oxy-ease-in\);/
    );
    expect(css).toMatch(
      /@media \(prefers-reduced-motion: reduce\) \{\s*\.oxy-rail \{\s*transition: none;/
    );
  });

  it("swaps the mark for the sidebar icon only on a pointer that hovers", () => {
    expect(css).toMatch(
      /@media \(hover: hover\) \{\s*\.oxy-rail:hover \.oxy-rail__toggle-mark \{\s*display: none;/
    );
  });
});
