// @vitest-environment jsdom
//
// ParentTabs: real links in a named nav, `aria-current="page"` on the open screen (its modes
// too), a plain left click handed to the app's router and anything else left to the browser.
// In the top bar (Bookkeeping's TopBarTabs, ported with its tests): the run that fits, the rest
// behind More, the open screen never hidden, a hidden count on More, and the menu's keys.
// jsdom lays nothing out, so every measured tab is 100px, More 40px, the bar `room` px.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { $, $$, click, fire, press, render } from "../ui.harness";
import { restoreLayout, stubLayout } from "./layout.harness";
import { type ParentTab, ParentTabs, type ParentTabsProps } from "./ParentTabs";

type Id = "a" | "b" | "c" | "d" | "e" | "f";
const LABELS: Record<Id, string> = {
  a: "Overview",
  b: "Reports",
  c: "Payroll",
  d: "Credits",
  e: "Counts",
  f: "Settings"
};
const tabs = (counts: Partial<Record<Id, number>> = {}): ParentTab<Id>[] =>
  (Object.keys(LABELS) as Id[]).map((id) => ({
    id,
    label: LABELS[id],
    href: `/app/${id}`,
    count: counts[id],
    countLabel: counts[id] ? `${counts[id]} to tag` : undefined
  }));

afterEach(() => {
  document.body.innerHTML = "";
  restoreLayout();
});

async function mount(props: Partial<ParentTabsProps<Id>> = {}) {
  await render(<ParentTabs label='Screen' tabs={tabs()} current='a' {...props} />);
}

const clickWith = (el: Element | null, init: MouseEventInit) => {
  const ev = new MouseEvent("click", { bubbles: true, cancelable: true, ...init });
  return fire(el, ev).then(() => ev);
};

describe("ParentTabs on the page", () => {
  it("is a nav of real links, named for the choice, the open screen marked current", async () => {
    await mount({ current: "b", placement: "page" });
    const nav = $("nav") as HTMLElement;
    expect(nav.getAttribute("aria-label")).toBe("Screen");
    const links = $$("nav a");
    expect(links.map((a) => a.getAttribute("href"))).toEqual(
      Object.keys(LABELS).map((k) => `/app/${k}`)
    );
    expect(
      links.filter((a) => a.getAttribute("aria-current") === "page").map((a) => a.textContent)
    ).toEqual(["Reports"]);
    expect($('[role="tab"]')).toBeNull();
    // Each link its own Tab stop.
    expect(links.every((a) => a.tabIndex === 0)).toBe(true);
  });

  it("keeps a tab lit for a screen that is one of its modes (`also`)", async () => {
    const list: ParentTab<string>[] = [
      { id: "stores", also: ["store"], label: "Stores", href: "/stores" },
      { id: "flags", label: "Flags", href: "/flags" }
    ];
    await render(<ParentTabs label='Records' tabs={list} current='store' />);
    expect($('a[aria-current="page"]')?.textContent).toBe("Stores");
  });

  it("hands a plain left click to onNavigate, and leaves ⌘, Ctrl, Shift and a middle click to the browser", async () => {
    const onNavigate = vi.fn();
    await mount({ onNavigate, placement: "page" });
    const reports = $$("nav a")[1];
    const plain = await clickWith(reports, {});
    expect(plain.defaultPrevented).toBe(true);
    expect(onNavigate).toHaveBeenCalledWith("b", expect.objectContaining({ href: "/app/b" }));
    for (const init of [{ metaKey: true }, { ctrlKey: true }, { shiftKey: true }, { button: 1 }]) {
      const ev = await clickWith(reports, init);
      expect(ev.defaultPrevented, JSON.stringify(init)).toBe(false);
    }
    expect(onNavigate).toHaveBeenCalledTimes(1);
  });

  it("lets the link navigate natively without onNavigate", async () => {
    await mount({ placement: "inline" });
    const ev = await clickWith($$("nav a")[2], {});
    expect(ev.defaultPrevented).toBe(false);
  });

  it("hides the page strip from the width its top-bar twin shows at", async () => {
    await mount({ placement: "page", topBarFrom: "xl" });
    expect($("nav")?.className).toContain("oxy-ui-parent-tabs--page-xl");
    document.body.innerHTML = "";
    await mount({ placement: "inline" });
    expect($("nav")?.className).not.toMatch(/--page-/);
  });
});

describe("ParentTabs in the top bar", () => {
  let room = 2000;
  beforeEach(() => {
    room = 2000;
    stubLayout({
      clientWidth: (el) => (el.getAttribute("data-placement") === "topbar" ? room : undefined),
      rect: (el) =>
        el.classList.contains("oxy-ui-parent-tabs__measure-tab")
          ? { left: 0, width: 100 }
          : el.classList.contains("oxy-ui-parent-tabs__measure-more")
            ? { left: 0, width: 40 }
            : undefined
    });
  });

  const top = (props: Partial<ParentTabsProps<Id>> = {}) =>
    mount({ placement: "topbar", ...props });
  /** The tabs drawn in the bar, in order (not the hidden measuring copy). */
  const barTabs = () => $$("nav > a").map((a) => a.textContent);
  const more = () => $("button.oxy-ui-parent-tabs__more");
  const menuItems = () => $$(".oxy-ui-parent-tabs__menu a");

  it("draws every screen, and no More, when they all fit", async () => {
    await top();
    expect(barTabs()).toEqual(Object.values(LABELS));
    expect(more()).toBeNull();
    expect($("nav")?.className).toContain("oxy-ui-parent-tabs--topbar-md");
  });

  it("keeps the run that fits beside More and puts the rest behind it", async () => {
    room = 440; // 4 × 100 + 40
    await top();
    expect(barTabs()).toEqual(["Overview", "Reports", "Payroll", "Credits"]);
    expect(more()?.getAttribute("aria-expanded")).toBe("false");
    expect(more()?.getAttribute("title")).toBe("More screens: Counts, Settings");
  });

  it("never hides the open screen: it goes last, and gives up a tab for it", async () => {
    room = 440;
    await top({ current: "f" });
    expect(barTabs()).toEqual(["Overview", "Reports", "Payroll", "Settings"]);
    expect($('nav > a[aria-current="page"]')?.textContent).toBe("Settings");
  });

  it("carries a hidden screen's count on More, in words for a screen reader", async () => {
    room = 440;
    await top({ tabs: tabs({ e: 3 }) });
    expect(more()?.querySelector(".oxy-ui-tab-count")?.textContent).toBe("3");
    expect(more()?.getAttribute("aria-label")).toBe("More screens, Counts: 3 to tag");
  });

  it("opens to the hidden screens as links; a plain click navigates and closes it", async () => {
    room = 440;
    const onNavigate = vi.fn();
    await top({ onNavigate });
    await click(more());
    expect(more()?.getAttribute("aria-expanded")).toBe("true");
    expect(menuItems().map((a) => [a.textContent, a.getAttribute("href")])).toEqual([
      ["Counts", "/app/e"],
      ["Settings", "/app/f"]
    ]);
    await click(menuItems()[1]);
    expect(onNavigate).toHaveBeenCalledWith("f", expect.anything());
    expect(menuItems()).toHaveLength(0);
  });

  it("leaves a ⌘-click in the menu to the browser, and still closes", async () => {
    room = 440;
    const onNavigate = vi.fn();
    await top({ onNavigate });
    await click(more());
    const ev = await clickWith(menuItems()[0], { metaKey: true });
    expect(onNavigate).not.toHaveBeenCalled();
    expect(ev.defaultPrevented).toBe(false);
    expect(menuItems()).toHaveLength(0);
  });

  it("opens from the keyboard onto its first screen; arrows wrap, End jumps, Escape returns to More", async () => {
    room = 440;
    await top();
    more()?.focus();
    await press("ArrowDown");
    expect(document.activeElement?.textContent).toBe("Counts");
    await press("ArrowDown");
    expect(document.activeElement?.textContent).toBe("Settings");
    await press("ArrowDown");
    expect(document.activeElement?.textContent).toBe("Counts");
    await press("End");
    expect(document.activeElement?.textContent).toBe("Settings");
    await press("Home");
    expect(document.activeElement?.textContent).toBe("Counts");
    await press("Escape");
    expect(menuItems()).toHaveLength(0);
    expect(document.activeElement).toBe(more());
  });

  it("opens onto its last screen with ArrowUp", async () => {
    room = 440;
    await top();
    more()?.focus();
    await press("ArrowUp");
    expect(document.activeElement?.textContent).toBe("Settings");
  });

  it("closes on a press anywhere else", async () => {
    room = 440;
    await top();
    await click(more());
    expect(menuItems()).toHaveLength(2);
    await fire(document.body, new Event("pointerdown", { bubbles: true }));
    expect(menuItems()).toHaveLength(0);
  });

  it("keeps the measuring copy out of the accessibility tree and the Tab order", async () => {
    await top();
    const copy = $(".oxy-ui-parent-tabs__measure") as HTMLElement;
    expect(copy.getAttribute("aria-hidden")).toBe("true");
    expect(copy.hasAttribute("inert")).toBe(true);
    expect(copy.querySelector("a, button")).toBeNull();
  });
});
