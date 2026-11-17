// @vitest-environment jsdom
//
// The person's light/dark mode in the wired shell, mounted for real against a fake server: with
// theme.css applied the shell applies the shell context's `mode`, keeps this origin's copy, re-reads
// it when the tab is shown again, and its rail switch writes a choice back (PUT). Without the theme,
// with a pin, on an older server or for a crew sign-in it applies nothing and draws no switch.

import * as React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  click,
  mount,
  type Page,
  servePage,
  setHidden,
  settle
} from "../custom-app/notifications.harness";
import { OXY_PREVIEW_MODE_CHOSEN_KEY, useOxyModePin } from "./mode";
import { NAV_SHEET_QUERY } from "./NavSheet";
import { OxyShell, type OxyShellProps } from "./OxyShell";
import type { ShellContextData } from "./shellContext";

const BASE: ShellContextData = {
  workspace: { id: "ws-1", name: "Default" },
  org: { id: "org-1", slug: "acme", name: "Poke House" },
  logo_url: null,
  apps: [{ id: "app-1", name: "Bookings", slug: "bookings", url: "/customer-apps/acme/bookings/" }],
  links: { home: "/", threads: "/threads" },
  user: { name: "Sam", email: "sam@example.com", id: "u-1", kind: "member" }
};

interface Call {
  method: string;
  url: string;
  body?: string;
}

// The switch's tooltip (Radix) measures itself when it opens; jsdom has no ResizeObserver.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

/** A screen that pins light without the shell's prop, inside an app that may show it. */
function Kiosk() {
  useOxyModePin("light");
  return <span>kiosk</span>;
}
function App({ kiosk }: { kiosk: boolean }) {
  return <OxyShell>{kiosk ? <Kiosk /> : "desk"}</OxyShell>;
}
/** The kiosk screen, then the person's own desk once they leave it, in one mounted shell. */
function LeaveKiosk() {
  const [kiosk, setKiosk] = React.useState(true);
  return (
    <OxyShell>
      {kiosk ? <Kiosk /> : "desk"}
      <button type='button' data-testid='leave-kiosk' onClick={() => setKiosk(false)}>
        leave
      </button>
    </OxyShell>
  );
}

let page: Page | undefined;
let calls: Call[] = [];
let serverMode: unknown = "dark";
let putStatus = 200;

const root = document.documentElement;
const $ = (sel: string) => document.querySelector(sel);
const switchEl = () => $('[data-testid="shell-mode-switch"]');

function context(overrides: Partial<ShellContextData> | "no-mode" = {}): ShellContextData {
  if (overrides === "no-mode") return { ...BASE };
  return { ...BASE, mode: serverMode as ShellContextData["mode"], ...overrides };
}

async function render(
  body: ShellContextData = context(),
  props: Partial<OxyShellProps> = {},
  ui?: React.ReactNode
): Promise<Page> {
  const fetcher = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    const method = init?.method ?? "GET";
    calls.push({ method, url, body: init?.body as string | undefined });
    if (url.includes("/shell-context/mode")) {
      if (method === "PUT") return new Response("{}", { status: putStatus });
      return new Response(JSON.stringify({ mode: serverMode }), { status: 200 });
    }
    return new Response(JSON.stringify(body), { status: 200 });
  }) as unknown as typeof fetch;
  page = await mount(ui ?? <OxyShell {...props}>app body</OxyShell>, fetcher);
  await settle();
  return page;
}

function theme(on: boolean) {
  if (on) root.style.setProperty("--oxy-theme-contract", "1");
  else root.style.removeProperty("--oxy-theme-contract");
}

const shown = () => ({
  mode: root.getAttribute("data-oxy-mode"),
  dark: root.classList.contains("dark")
});

beforeEach(() => {
  vi.useFakeTimers();
  servePage();
  calls = [];
  serverMode = "dark";
  putStatus = 200;
  localStorage.clear();
  root.removeAttribute("data-oxy-mode");
  root.removeAttribute("data-oxy-mode-pin");
  root.className = "";
  theme(true);
});

afterEach(async () => {
  await page?.unmount();
  page = undefined;
  theme(false);
  vi.restoreAllMocks();
  vi.useRealTimers();
});

describe("OxyShell and the person's mode, with the theme", () => {
  it("applies the shell context's mode and keeps this origin's copy", async () => {
    await render();
    expect(shown()).toEqual({ mode: "dark", dark: true });
    expect(localStorage.getItem("oxy-person-mode")).toBe("dark");
    expect(switchEl()?.getAttribute("aria-label")).toBe("Appearance: Dark");
  });

  it("shows Oxygen's default, light, for a person who has not chosen", async () => {
    root.classList.add("dark");
    serverMode = null;
    await render();
    expect(shown()).toEqual({ mode: "light", dark: false });
    expect(localStorage.getItem("oxy-person-mode")).toBe("light");
  });

  it("writes a choice from the rail switch, and Oxygen's copy is what it writes", async () => {
    await render();
    await click(switchEl());
    const items = [...document.querySelectorAll('[role="menuitemradio"]')];
    expect(items.map((i) => [i.textContent, i.getAttribute("aria-checked")])).toEqual([
      ["Light", "false"],
      ["Dark", "true"],
      ["System", "false"]
    ]);
    await click($('[data-testid="shell-mode-light"]'));
    expect(shown()).toEqual({ mode: "light", dark: false });
    const put = calls.find((c) => c.method === "PUT");
    expect(put).toEqual({
      method: "PUT",
      url: "/api/projects/proj-1/shell-context/mode",
      body: JSON.stringify({ mode: "light" })
    });
    expect(localStorage.getItem("oxy-person-mode")).toBe("light");
    expect($('[role="menu"]')).toBeNull();
  });

  it("puts the old mode back when the server refuses the write", async () => {
    putStatus = 403;
    await render();
    await click(switchEl());
    await click($('[data-testid="shell-mode-light"]'));
    await settle();
    expect(shown()).toEqual({ mode: "dark", dark: true });
    expect(localStorage.getItem("oxy-person-mode")).toBe("dark");
  });

  it("re-reads the mode when the tab is shown again, so a change made in Oxygen shows up", async () => {
    await render();
    serverMode = "light";
    await React.act(async () => setHidden(true));
    expect(calls.filter((c) => c.url.endsWith("/shell-context/mode"))).toHaveLength(0);
    await React.act(async () => setHidden(false));
    await settle();
    expect(calls.filter((c) => c.url.endsWith("/shell-context/mode"))).toHaveLength(1);
    expect(shown()).toEqual({ mode: "light", dark: false });
  });

  it("follows the device under system", async () => {
    serverMode = "system";
    const listeners: (() => void)[] = [];
    let dark = false;
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: () => ({
        get matches() {
          return dark;
        },
        addEventListener: (_: string, l: () => void) => listeners.push(l),
        removeEventListener: () => {}
      })
    });
    await render();
    expect(shown().mode).toBe("light");
    dark = true;
    await React.act(async () => {
      for (const l of listeners) l();
    });
    expect(shown()).toEqual({ mode: "dark", dark: true });
  });

  it("puts the switch in the nav sheet's bottom on a phone", async () => {
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: (q: string) => ({
        matches: q === NAV_SHEET_QUERY,
        addEventListener: () => {},
        removeEventListener: () => {}
      })
    });
    await render();
    await click($('[data-testid="nav-sheet-trigger"]'));
    const seg = $('.oxy-navsheet__bottom [role="radiogroup"]');
    expect(seg?.getAttribute("aria-label")).toBe("Appearance");
    await click(seg?.querySelector('[data-testid="shell-mode-system"]') ?? null);
    expect(calls.find((c) => c.method === "PUT")?.body).toBe(JSON.stringify({ mode: "system" }));
  });
});

describe("OxyShell and the person's mode: when it does nothing", () => {
  it("without the theme: no mode applied, no switch, no rail bottom", async () => {
    theme(false);
    await render();
    expect(shown()).toEqual({ mode: null, dark: false });
    expect(switchEl()).toBeNull();
    expect($(".oxy-rail__bottom")).toBeNull();
    expect(localStorage.getItem("oxy-person-mode")).toBe("dark");
  });

  it("on an older server (no mode field): the app keeps its own mode", async () => {
    root.classList.add("dark");
    await render(context("no-mode"));
    expect(shown()).toEqual({ mode: null, dark: true });
    expect(switchEl()).toBeNull();
    expect($(".oxy-rail__bottom")).toBeNull();
    await React.act(async () => setHidden(true));
    await React.act(async () => setHidden(false));
    await settle();
    expect(calls.some((c) => c.url.includes("/shell-context/mode"))).toBe(false);
  });

  it("for a crew sign-in: no switch, nothing applied, read or written", async () => {
    // The server leaves `mode` out for a crew sign-in; the shell holds even if one arrived.
    await render(context({ user: { name: "Kai", email: "", id: "u-2", kind: "frontline" } }));
    expect(shown()).toEqual({ mode: null, dark: false });
    expect(switchEl()).toBeNull();
    await React.act(async () => setHidden(true));
    await React.act(async () => setHidden(false));
    await settle();
    expect(calls.some((c) => c.url.includes("/shell-context/mode"))).toBe(false);
  });

  it("with modePin: the pinned mode shows, the switch hides, and unmounting restores", async () => {
    await render(context(), { modePin: "light" });
    expect(shown()).toEqual({ mode: "light", dark: false });
    expect(root.getAttribute("data-oxy-mode-pin")).toBe("light");
    expect(switchEl()).toBeNull();
    await page?.unmount();
    page = undefined;
    expect(root.hasAttribute("data-oxy-mode-pin")).toBe(false);
  });

  // A pinned screen is nobody's (a store's shared tablet): the person signed in there must leave
  // no mode in this origin's copy, which the pre-paint applies on the next load.
  it("with modePin: this origin's copy is left as it was", async () => {
    await render(context(), { modePin: "light" });
    expect(localStorage.getItem("oxy-person-mode")).toBeNull();
    await page?.unmount();
    page = undefined;
    localStorage.setItem("oxy-person-mode", "light");
    await render(context(), { modePin: "light" });
    await React.act(async () => setHidden(true));
    await React.act(async () => setHidden(false));
    await settle();
    expect(localStorage.getItem("oxy-person-mode")).toBe("light");
  });

  it("a pin in index.html leaves the copy alone too", async () => {
    root.setAttribute("data-oxy-mode-pin", "light");
    await render();
    expect(shown().mode).toBe("light");
    expect(localStorage.getItem("oxy-person-mode")).toBeNull();
  });

  it("a screen's pin leaves the copy alone, and the person's own screen keeps it once the pin lifts", async () => {
    await render(context(), {}, <LeaveKiosk />);
    expect(shown().mode).toBe("light");
    expect(localStorage.getItem("oxy-person-mode")).toBeNull();
    await click($('[data-testid="leave-kiosk"]'));
    expect(shown()).toEqual({ mode: "dark", dark: true });
    expect(localStorage.getItem("oxy-person-mode")).toBe("dark");
  });

  it("a pin in index.html holds too, and a screen's pin gives it back when it goes", async () => {
    root.setAttribute("data-oxy-mode-pin", "dark");
    serverMode = "light";
    await render(context(), {}, <App kiosk />);
    expect(shown().mode).toBe("light");
    expect(switchEl()).toBeNull();
    await page?.unmount();
    page = undefined;
    expect(root.getAttribute("data-oxy-mode-pin")).toBe("dark");
    await render(context(), {}, <App kiosk={false} />);
    expect(shown()).toEqual({ mode: "dark", dark: true });
    expect(switchEl()).toBeNull();
  });
});

// Mars, 16 Nov: a choice made on a staging or sandbox host must not write the person's real mode.
// The switch still works there, for that preview only: this origin's copy, never the server.
describe("OxyShell and the person's mode on a staging or sandbox host", () => {
  const puts = () => calls.filter((c) => c.method === "PUT");
  const reads = () =>
    calls.filter((c) => c.method === "GET" && c.url.endsWith("/shell-context/mode"));

  it.each(["staging", "dev-mars"] as const)(
    "%s: shows the person's mode until a choice, then keeps the choice here and never sends it",
    async (environment) => {
      servePage("app-1", environment);
      await render();
      expect(shown()).toEqual({ mode: "dark", dark: true });
      await click(switchEl());
      await click($('[data-testid="shell-mode-light"]'));
      await settle();
      expect(puts()).toEqual([]);
      expect(shown()).toEqual({ mode: "light", dark: false });
      expect(localStorage.getItem("oxy-person-mode")).toBe("light");
      expect(localStorage.getItem(OXY_PREVIEW_MODE_CHOSEN_KEY)).toBe("1");

      // No snap back: a tab shown again keeps the preview's choice, and reads nothing.
      await React.act(async () => setHidden(true));
      await React.act(async () => setHidden(false));
      await settle();
      expect(reads()).toEqual([]);
      expect(shown()).toEqual({ mode: "light", dark: false });

      // Nor on the next load, when the shell context says the person's dark again.
      await page?.unmount();
      page = undefined;
      await render();
      expect(shown()).toEqual({ mode: "light", dark: false });
      expect(puts()).toEqual([]);
    }
  );

  it("production still writes the choice to the person", async () => {
    servePage("app-1", "production");
    await render();
    await click(switchEl());
    await click($('[data-testid="shell-mode-light"]'));
    expect(puts().map((c) => c.body)).toEqual([JSON.stringify({ mode: "light" })]);
    expect(localStorage.getItem(OXY_PREVIEW_MODE_CHOSEN_KEY)).toBeNull();
  });
});

// The browser's own chrome follows the page: with the theme, the shell keeps every
// `<meta name="theme-color">` on the page ground of the mode in force, pins included.
describe('OxyShell and <meta name="theme-color">', () => {
  let style: HTMLStyleElement;
  const meta = () => document.head.querySelectorAll('meta[name="theme-color"]');

  beforeEach(() => {
    style = document.createElement("style");
    style.textContent = ":root { --background: #f6f6f7; } :root.dark { --background: #0a0a0a; }";
    document.head.appendChild(style);
    for (const m of meta()) m.remove();
  });
  afterEach(() => {
    style.remove();
    for (const m of meta()) m.remove();
  });

  it("follows the mode, a switch and a pin, and adds a meta where the page has none", async () => {
    await render();
    expect([...meta()].map((m) => m.getAttribute("content"))).toEqual(["#0a0a0a"]);
    await click(switchEl());
    await click($('[data-testid="shell-mode-light"]'));
    expect(meta()[0]?.getAttribute("content")).toBe("#f6f6f7");
    await page?.unmount();
    page = undefined;
    serverMode = "dark";
    await render(context(), { modePin: "light" });
    expect(meta()[0]?.getAttribute("content")).toBe("#f6f6f7");
  });

  it("follows a mode the app sets on <html> itself", async () => {
    serverMode = "light";
    await render();
    expect(meta()[0]?.getAttribute("content")).toBe("#f6f6f7");
    await React.act(async () => {
      root.classList.add("dark");
      root.setAttribute("data-oxy-mode", "dark");
    });
    await settle();
    expect(meta()[0]?.getAttribute("content")).toBe("#0a0a0a");
  });

  it("updates the page's own metas", async () => {
    document.head.insertAdjacentHTML(
      "beforeend",
      '<meta name="theme-color" content="#ffffff"><meta name="theme-color" media="(prefers-color-scheme: dark)" content="#000000">'
    );
    await render();
    expect([...meta()].map((m) => m.getAttribute("content"))).toEqual(["#0a0a0a", "#0a0a0a"]);
  });

  it("leaves it alone without the theme", async () => {
    theme(false);
    document.head.insertAdjacentHTML("beforeend", '<meta name="theme-color" content="#ffffff">');
    await render();
    expect([...meta()].map((m) => m.getAttribute("content"))).toEqual(["#ffffff"]);
  });
});
