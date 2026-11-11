// @vitest-environment jsdom
//
// `<OxyShell>` on a phone: the rail's names are hover tooltips a touch screen
// never shows, so below 768px (and on a phone on its side) the shell swaps the
// rail for a menu button that opens the same entries, named, in a drawer. Mounted for real against a fake
// shell-context; the breakpoint itself lives in shell.css, checked as text.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import * as React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { click, mount, type Page, servePage, settle } from "../custom-app/notifications.harness";
import { NAV_SHEET_QUERY } from "./NavSheet";
import { OxyShell, type OxyShellProps } from "./OxyShell";
import { PHONE_LAYOUT_QUERY } from "./phone/usePhoneLayout";
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

let page: Page | undefined;

async function render(props: Partial<OxyShellProps> = {}, status = 200): Promise<Page> {
  const fetcher = (async () =>
    new Response(JSON.stringify(SHELL_CONTEXT), { status })) as unknown as typeof fetch;
  page = await mount(<OxyShell {...props}>app body</OxyShell>, fetcher);
  await settle();
  return page;
}

const $ = (sel: string) => document.querySelector(sel);
const trigger = () => $('[data-testid="nav-sheet-trigger"]');
const sheet = () => $('[data-testid="nav-sheet"]');

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

describe("OxyShell nav sheet", () => {
  it("lists every rail entry by name, the current app marked", async () => {
    await render();
    expect($(".oxy-shell")?.classList.contains("oxy-shell--navsheet")).toBe(true);
    expect(sheet()).toBeNull();
    await click(trigger());
    const labels = [...document.querySelectorAll(".oxy-navsheet__label")];
    expect(labels.map((l) => l.textContent)).toEqual(["HQ", "Chat", "Bookings", "Command Center"]);
    const current = $('[data-testid="nav-sheet-rail-app-bookings"]');
    expect(current?.getAttribute("aria-current")).toBe("page");
    expect(current?.getAttribute("href")).toBe("/customer-apps/acme/bookings/");
    // The letter mark is decorative, so the row's text is just its name.
    expect(current?.querySelector(".oxy-navsheet__icon")?.getAttribute("aria-hidden")).toBe("true");
    expect(sheet()?.getAttribute("aria-label")).toBe("Poke House");
  });

  it("closes on a pick, on the overlay, and on Escape", async () => {
    await render();
    await click(trigger());
    await click($('[data-testid="nav-sheet-rail-chat"]'));
    expect(sheet()).toBeNull();

    await click(trigger());
    await click($(".oxy-navsheet__overlay"));
    expect(sheet()).toBeNull();

    await click(trigger());
    await React.act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    });
    expect(sheet()).toBeNull();
    // Focus returns to the menu button rather than stranding on <body>.
    expect(document.activeElement).toBe(trigger());
  });

  it("keeps Tab inside the open drawer", async () => {
    await render();
    await click(trigger());
    const panel = sheet() as HTMLElement;
    const close = panel.querySelector<HTMLElement>(".oxy-navsheet__close");
    const rows = panel.querySelectorAll<HTMLElement>(".oxy-navsheet__row");
    const last = rows[rows.length - 1];
    expect(document.activeElement).toBe(close);
    const tab = async (shiftKey = false) =>
      React.act(async () => {
        const e = new KeyboardEvent("keydown", { key: "Tab", shiftKey, cancelable: true });
        window.dispatchEvent(e);
      });
    await tab(true);
    expect(document.activeElement).toBe(last);
    await tab();
    expect(document.activeElement).toBe(close);
    // Focus that got out anyway (a click on the page) is pulled back in.
    ($('[data-testid="topbar-workspace-link"]') as HTMLElement).focus();
    await tab();
    expect(document.activeElement).toBe(close);
  });

  /// An app's own `fixed` bar at the same z-index must paint under the
  /// drawer, which it does only when the drawer comes later in the DOM.
  it("renders the drawer after the app content", async () => {
    await render();
    await click(trigger());
    const main = $(".oxy-shell__main") as Node;
    const drawer = $(".oxy-navsheet") as Node;
    expect(drawer.parentElement).toBe($(".oxy-shell"));
    expect(main.compareDocumentPosition(drawer) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("closes when the window leaves the drawer's query", async () => {
    const listeners: Array<() => void> = [];
    const asked: string[] = [];
    const query = {
      matches: true,
      addEventListener: (_: string, fn: () => void) => listeners.push(fn),
      removeEventListener: () => {}
    };
    vi.stubGlobal("matchMedia", (q: string) => {
      asked.push(q);
      return query;
    });
    await render();
    await click(trigger());
    expect(sheet()).not.toBeNull();
    expect(asked).toContain(NAV_SHEET_QUERY);
    query.matches = false;
    await React.act(async () => {
      for (const fn of listeners) fn();
    });
    expect(sheet()).toBeNull();
    vi.unstubAllGlobals();
  });

  /// With no top bar there is nowhere to put the menu button, so the rail
  /// must stay at every width — a phone is never left without navigation.
  it("keeps the rail when the top bar is hidden", async () => {
    await render({ hideTopBar: true });
    expect($(".oxy-shell")?.classList.contains("oxy-shell--navsheet")).toBe(false);
    expect(trigger()).toBeNull();
    expect($('[data-testid="shell-rail"]')).not.toBeNull();
  });

  it("renders no menu when the shell is degraded", async () => {
    await render({}, 500);
    expect(trigger()).toBeNull();
  });
});

describe("shell.css nav sheet breakpoint", () => {
  const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "shell.css"), "utf8");
  const start = css.indexOf(`@media ${NAV_SHEET_QUERY} {`);
  const block = css.slice(start, css.indexOf("\n}\n", start));

  it("hides the rail and shows the menu button under NAV_SHEET_QUERY only", () => {
    expect(start).toBeGreaterThanOrEqual(0);
    expect(block).toMatch(/\.oxy-shell--navsheet > \.oxy-rail \{\s*display: none;/);
    expect(block).toMatch(/\.oxy-navsheet-trigger \{\s*display: inline-flex;/);
    expect(css).toMatch(/\.oxy-navsheet-trigger \{\s*display: none;/);
  });

  // A phone on its side (844x390) is wider than 768px, and its rail's 32px items
  // were no touch target. It takes the drawer by the phone layout's landscape
  // half; a touch tablet (taller) and a desktop (fine pointer) keep the rail.
  it("covers a phone on its side, by the phone layout's landscape half", () => {
    expect(NAV_SHEET_QUERY).toBe(
      "(max-width: 767.98px), (pointer: coarse) and (max-height: 500px)"
    );
    expect(PHONE_LAYOUT_QUERY.split(", ")[1]).toBe(NAV_SHEET_QUERY.split(", ")[1]);
  });

  // Where the rail stays on a phone on its side (`hideTopBar`, or an app's own
  // ShellRail), its items are 44px, and padding no longer squeezes them.
  it("gives a rail on a phone on its side 44px items", () => {
    const head = "@media (pointer: coarse) and (max-height: 500px) {";
    const at = css.indexOf(head);
    expect(at).toBeGreaterThanOrEqual(0);
    const rail = css.slice(at, css.indexOf("\n}\n", at));
    expect(rail).toMatch(/\.oxy-rail__item \{\s*height: 2\.75rem;\s*width: 2\.75rem;/);
    expect(rail).toMatch(
      /\.oxy-rail__nav,\s*\.oxy-rail__bottom \{\s*padding-left: 0;\s*padding-right: 0;/
    );
  });
});
