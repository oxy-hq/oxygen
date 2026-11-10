// @vitest-environment jsdom
//
// Ask Oxygen on a phone covers the screen instead of docking beside the app,
// which on a 390px screen left the app a ~60px column. Against jsdom's real
// session history: the phone's Back closes it, the app behind is inert while it
// is open and not after, the composer follows the visual viewport (the keyboard).
// A tablet and a desktop keep the docked panel exactly as before. The look itself
// lives in shell.css and is read as text here.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import * as React from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { servePage } from "../custom-app/notifications.harness";
import { OxyAppProvider } from "../custom-app/react";
import { AskDock } from "./AskDock";

const PAGE = "http://localhost:3000/page";
const CSS = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "shell.css"), "utf8");

let setOpen: (open: boolean) => void = () => {};
let closes = 0;

function Shell() {
  const [open, set] = React.useState(false);
  setOpen = set;
  return (
    <div className='oxy-shell'>
      <div className='oxy-shell__content-col' data-testid='app'>
        <button type='button' onClick={() => set(true)} data-testid='ask'>
          Ask
        </button>
      </div>
      <AskDock
        agentId='agents/ask.agentic.yml'
        open={open}
        onClose={() => {
          closes += 1;
          set(false);
        }}
      />
    </div>
  );
}

const $ = (sel: string) => document.querySelector(sel);
const dock = () => $(".oxy-askdock") as HTMLElement;
const app = () => $('[data-testid="app"]') as HTMLElement;
const here = () => {
  const state = window.history.state as { page?: string; oxyPhoneSheetDepth?: number } | null;
  if (state?.oxyPhoneSheetDepth) return "ask";
  return state?.page;
};

async function wait(ms = 30) {
  await React.act(async () => {
    await new Promise((r) => setTimeout(r, ms));
  });
}

async function toggle(open: boolean) {
  await React.act(async () => setOpen(open));
  await wait();
}

let root: ReturnType<typeof createRoot> | undefined;
let phone = true;

beforeEach(async () => {
  phone = true;
  closes = 0;
  vi.stubGlobal("matchMedia", (media: string) => ({
    matches: phone,
    media,
    addEventListener() {},
    removeEventListener() {}
  }));
  // jsdom has no `scrollTo`; the dock's stick-to-bottom effect calls it.
  Element.prototype.scrollTo = () => {};
  servePage();
  window.history.pushState({ page: "base" }, "", PAGE);
  await new Promise((r) => setTimeout(r, 0));
});

afterEach(async () => {
  await React.act(async () => root?.unmount());
  root = undefined;
  await wait();
  vi.unstubAllGlobals();
  document.body.innerHTML = "";
});

async function render() {
  const fetcher = (async () =>
    new Response(JSON.stringify([]), { status: 200 })) as unknown as typeof fetch;
  const container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await React.act(async () => {
    root?.render(
      <OxyAppProvider fetcher={fetcher}>
        <Shell />
      </OxyAppProvider>
    );
  });
  await wait(50);
}

describe("Ask Oxygen on a phone", () => {
  it("covers the screen with the app inert behind it, and Back closes it", async () => {
    await render();
    await toggle(true);
    expect(dock().classList.contains("oxy-askdock--phone")).toBe(true);
    expect(dock().classList.contains("oxy-askdock--closed")).toBe(false);
    expect(app().hasAttribute("inert")).toBe(true);
    expect(here()).toBe("ask");

    await React.act(async () => window.history.back());
    await wait(60);
    expect(closes).toBe(1);
    expect(dock().classList.contains("oxy-askdock--closed")).toBe(true);
    expect(app().hasAttribute("inert")).toBe(false);
    expect(here()).toBe("base");
  });

  it("closes from its own close button and takes the entry away", async () => {
    await render();
    const before = window.history.length;
    await toggle(true);
    expect(window.history.length).toBe(before + 1);
    await React.act(async () => {
      ($('[aria-label="Close Ask Oxygen"]') as HTMLButtonElement).click();
    });
    await wait(60);
    expect(dock().classList.contains("oxy-askdock--closed")).toBe(true);
    expect(app().hasAttribute("inert")).toBe(false);
    expect(here()).toBe("base");
  });

  it("cannot trap Back when the app pushes its own entry while the panel is open", async () => {
    await render();
    await toggle(true);
    await React.act(async () => window.history.pushState({ page: "app" }, "", `${PAGE}?x=1`));
    await React.act(async () => window.history.back());
    await wait(60);
    expect(here()).toBe("ask");
    expect(dock().classList.contains("oxy-askdock--closed")).toBe(false);
    const length = window.history.length;
    await React.act(async () => window.history.back());
    await wait(60);
    expect(dock().classList.contains("oxy-askdock--closed")).toBe(true);
    expect(here()).toBe("base");
    expect(window.history.length).toBe(length);
  });

  it("follows the visual viewport, so the composer stays above the keyboard", async () => {
    const view = Object.assign(new EventTarget(), { height: 844, offsetTop: 0 });
    vi.stubGlobal("visualViewport", view);
    await render();
    await toggle(true);
    expect(dock().hasAttribute("data-fit")).toBe(true);
    expect(dock().style.getPropertyValue("--oxy-askdock-vh")).toBe("844px");

    // The keyboard comes up: the visible part shrinks and scrolls.
    await React.act(async () => {
      Object.assign(view, { height: 470, offsetTop: 120 });
      view.dispatchEvent(new Event("resize"));
    });
    expect(dock().style.getPropertyValue("--oxy-askdock-vh")).toBe("470px");
    expect(dock().style.getPropertyValue("--oxy-askdock-top")).toBe("120px");

    await toggle(false);
    expect(dock().hasAttribute("data-fit")).toBe(false);
  });

  it("is a fixed full-screen layer in shell.css, with 44px controls and a 16px composer", () => {
    const rule = (sel: string) => {
      const at = CSS.indexOf(`${sel} {`);
      expect(at, sel).toBeGreaterThan(-1);
      return CSS.slice(at, CSS.indexOf("}", at));
    };
    const open = rule(".oxy-askdock--phone:not(.oxy-askdock--closed)");
    expect(open).toMatch(/position: fixed/);
    expect(open).toMatch(/inset: 0/);
    expect(open).toMatch(/width: auto/);
    expect(open).toMatch(/z-index: 55/);
    expect(rule(".oxy-askdock--phone[data-fit]:not(.oxy-askdock--closed)")).toMatch(
      /height: var\(--oxy-askdock-vh\)/
    );
    expect(rule(".oxy-askdock--phone .oxy-askdock__iconbtn")).toMatch(/width: 44px/);
    expect(
      rule(
        ".oxy-askdock--phone .oxy-askdock__input,\n.oxy-askdock--phone .oxy-askdock__history-searchinput"
      )
    ).toMatch(/font-size: 16px/);
  });
});

describe("Ask Oxygen on a tablet or a desktop", () => {
  it("stays the docked side panel: no cover, nothing inert, no history entry", async () => {
    phone = false;
    await render();
    const before = window.history.length;
    await toggle(true);
    expect(dock().classList.contains("oxy-askdock--phone")).toBe(false);
    expect(dock().classList.contains("oxy-askdock--closed")).toBe(false);
    expect(app().hasAttribute("inert")).toBe(false);
    expect(window.history.length).toBe(before);
    expect(here()).toBe("base");
  });
});
