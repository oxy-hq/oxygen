// @vitest-environment jsdom
//
// 2.31.1: `<OxyShell>` gives a `Stat`'s tip its touch form on a phone. Before, only an app that
// wrapped its phone layout in `TapHintFrame` got the kit's tap ⓘ on the label line; every other
// app's phone drew the desk ⓘ with a hover bubble, which a finger cannot open. Mounted for real
// against a fake shell-context, the window's media answered for a device (width, height, pointer)
// by the queries' own words, so the phone, the phone on its side, the touch tablet and the desk are
// each what `usePhoneLayout()` makes of them. Covered: the tap form and a tap that opens and
// closes it; the keyboard's one Tab stop, focus opening it and Escape closing it there; a coarse
// pointer (a phone on its side takes the tap form, a touch tablet on its desk layout keeps the
// desk); an app's own frame winning inside the shell and around it; and the swap across the
// breakpoint re-rendering the tip in place, the app's state kept.

import * as React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mount, type Page, servePage, settle } from "../custom-app/notifications.harness";
import { TapHintFrame } from "../ui/containers";
import { Stat } from "../ui/figures";
import { OxyShell } from "./OxyShell";
import type { ShellContextData } from "./shellContext";

const SHELL_CONTEXT: ShellContextData = {
  workspace: { id: "ws-1", name: "Default" },
  org: { id: "org-1", slug: "acme", name: "Acme" },
  logo_url: null,
  apps: [{ id: "app-1", name: "Bookings", slug: "bookings", url: "/customer-apps/acme/bookings/" }],
  links: { home: "/", threads: "/threads" },
  user: null
};

interface Device {
  width: number;
  height: number;
  coarse: boolean;
}
const PHONE: Device = { width: 390, height: 844, coarse: true };
const PHONE_ON_ITS_SIDE: Device = { width: 844, height: 390, coarse: true };
const TOUCH_TABLET: Device = { width: 1024, height: 768, coarse: true };
const DESK: Device = { width: 1280, height: 800, coarse: false };

/** Whether `query` holds on `d`: each comma-separated alternative, every feature in it. */
function holds(query: string, d: Device): boolean {
  return query.split(",").some((alt) => {
    const features = [...alt.matchAll(/\(\s*([a-z-]+)\s*:\s*([^)]+?)\s*\)/g)];
    return (
      features.length > 0 &&
      features.every(([, name, value]) => {
        const px = Number.parseFloat(value);
        if (name === "max-width") return d.width <= px;
        if (name === "min-width") return d.width >= px;
        if (name === "max-height") return d.height <= px;
        if (name === "min-height") return d.height >= px;
        if (name === "pointer" || name === "any-pointer")
          return value === (d.coarse ? "coarse" : "fine");
        if (name === "hover" || name === "any-hover")
          return value === (d.coarse ? "none" : "hover");
        return false;
      })
    );
  });
}

let device = DESK;
const listeners = new Set<() => void>();
/** Become `next`, and tell every media query, as rotating a phone or resizing a window does. */
async function become(next: Device) {
  device = next;
  await React.act(async () => {
    for (const fn of listeners) fn();
  });
}

let page: Page | undefined;
async function render(ui: React.ReactNode, on: Device) {
  device = on;
  const fetcher = (async () =>
    new Response(JSON.stringify(SHELL_CONTEXT), { status: 200 })) as unknown as typeof fetch;
  page = await mount(ui, fetcher);
  await settle();
}

const tip = <Stat label='Income' value='$133k' tip='Census ACS, 2023.' boxed />;
const $ = (sel: string) => document.querySelector(sel) as HTMLElement | null;
const tile = () => $(".oxy-ui-stat") as HTMLElement;
const label = () => tile().firstElementChild as HTMLElement;
const tapMark = () => $(".oxy-phone-hint") as HTMLButtonElement | null;
const bubble = () => $("[role='tooltip']");
async function tap(el: Element) {
  await React.act(async () => {
    el.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, pointerType: "touch" }));
    el.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
}
/** The desk's form: the hint over the tile, and its ⓘ on the figure's line. */
const deskForm = () => ({
  hoverAnchor: tile().parentElement?.classList.contains("oxy-ui-hover-hint") ?? false,
  mark: $(".oxy-ui-stat__mark")?.getAttribute("aria-label") ?? null,
  tapMark: tapMark() !== null
});

/** An app's own state: a count that a remount would lose. */
function Counter() {
  const [n, setN] = React.useState(0);
  return (
    <button type='button' data-count={n} onClick={() => setN(n + 1)}>
      more
    </button>
  );
}

beforeEach(() => {
  vi.useFakeTimers();
  servePage();
  vi.stubGlobal("matchMedia", (query: string) => ({
    get matches() {
      return holds(query, device);
    },
    media: query,
    addEventListener: (_: string, fn: () => void) => listeners.add(fn),
    removeEventListener: (_: string, fn: () => void) => listeners.delete(fn),
    addListener: (fn: () => void) => listeners.add(fn),
    removeListener: (fn: () => void) => listeners.delete(fn)
  }));
});

afterEach(async () => {
  await page?.unmount();
  page = undefined;
  listeners.clear();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  vi.useRealTimers();
  document.body.innerHTML = "";
});

describe("OxyShell on a phone: a Stat's tip is a tap ⓘ with no frame from the app", () => {
  it("draws the kit's tap ⓘ after the label, and a tap opens the tip and a second closes it", async () => {
    await render(<OxyShell>{tip}</OxyShell>, PHONE);
    expect(deskForm()).toEqual({ hoverAnchor: false, mark: null, tapMark: true });
    const mark = tapMark() as HTMLButtonElement;
    expect(label().contains(mark)).toBe(true);
    expect(label().classList).toContain("oxy-ui-stat__label-row");
    expect(mark.getAttribute("aria-label")).toBe("About Income");
    expect(mark.getAttribute("aria-expanded")).toBe("false");
    await tap(mark);
    expect(bubble()?.textContent).toBe("Census ACS, 2023.");
    expect(mark.getAttribute("aria-expanded")).toBe("true");
    expect(mark.getAttribute("aria-describedby")).toBe(bubble()?.id);
    // Inside the shell's scope, so its tokens and a dark mode reach the bubble.
    expect($(".oxy-shell")?.contains(bubble())).toBe(true);
    await tap(mark);
    expect(bubble()).toBeNull();
  });

  it("is one Tab stop: keyboard focus opens the tip, and Escape closes it with focus kept", async () => {
    await render(<OxyShell>{tip}</OxyShell>, PHONE);
    const tabStops = [...tile().querySelectorAll("button, [href], [tabindex]")];
    expect(tabStops).toEqual([tapMark()]);
    // jsdom has no focus-visible heuristic: a key press makes focus visible in a browser.
    const original = Element.prototype.matches;
    vi.spyOn(Element.prototype, "matches").mockImplementation(function (this: Element, s) {
      return s === ":focus-visible" ? true : original.call(this, s);
    });
    const mark = tapMark() as HTMLButtonElement;
    await React.act(async () => mark.focus());
    expect(bubble()?.textContent).toBe("Census ACS, 2023.");
    await React.act(async () => {
      mark.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    expect(bubble()).toBeNull();
    expect(document.activeElement).toBe(mark);
  });

  it("follows the phone layout: a coarse pointer on a phone on its side taps, a touch tablet keeps the desk", async () => {
    await render(<OxyShell>{tip}</OxyShell>, PHONE_ON_ITS_SIDE);
    expect(deskForm()).toEqual({ hoverAnchor: false, mark: null, tapMark: true });
    await tap(tapMark() as HTMLButtonElement);
    expect(bubble()?.textContent).toBe("Census ACS, 2023.");
    await page?.unmount();
    // A tablet (coarse, but wide and tall) is on its desk layout, which stays as it was.
    await render(<OxyShell>{tip}</OxyShell>, TOUCH_TABLET);
    expect(deskForm()).toEqual({ hoverAnchor: true, mark: "About Income", tapMark: false });
    await page?.unmount();
    await render(<OxyShell>{tip}</OxyShell>, DESK);
    expect(deskForm()).toEqual({ hoverAnchor: true, mark: "About Income", tapMark: false });
  });

  it("leaves an app's own frame alone, inside the shell or around it", async () => {
    const appFrame = (name: string, text: string) => (
      <button type='button' data-app-hint='' aria-label={name}>
        {text}
      </button>
    );
    const appHint = () => $("[data-app-hint]")?.getAttribute("aria-label") ?? null;
    for (const on of [PHONE, DESK]) {
      await render(
        <OxyShell>
          <TapHintFrame.Provider value={appFrame}>{tip}</TapHintFrame.Provider>
        </OxyShell>,
        on
      );
      expect([appHint(), tapMark()]).toEqual(["About Income", null]);
      await page?.unmount();
      await render(
        <TapHintFrame.Provider value={appFrame}>
          <OxyShell>{tip}</OxyShell>
        </TapHintFrame.Provider>,
        on
      );
      expect([appHint(), tapMark()]).toEqual(["About Income", null]);
      await page?.unmount();
    }
    page = undefined;
  });

  it("swaps the tip's form in place across the breakpoint, the app's state kept", async () => {
    await render(
      <OxyShell>
        <Counter />
        {tip}
      </OxyShell>,
      DESK
    );
    await React.act(async () => {
      $("[data-count]")?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect($("[data-count]")?.dataset.count).toBe("1");
    await become(PHONE);
    expect(deskForm()).toEqual({ hoverAnchor: false, mark: null, tapMark: true });
    expect($("[data-count]")?.dataset.count).toBe("1");
    await become(DESK);
    expect(deskForm()).toEqual({ hoverAnchor: true, mark: "About Income", tapMark: false });
    expect($("[data-count]")?.dataset.count).toBe("1");
  });
});
