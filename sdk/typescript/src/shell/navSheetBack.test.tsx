// @vitest-environment jsdom
//
// The phone's Back with the shell's drawer open, against jsdom's real session
// history. `here` is the mark the page's own entry carries: if it reads back, the
// page is where it was. A real page's load cannot run in jsdom, which reports it
// on its virtual console, so that is what says an anchor's pick navigated.

import * as React from "react";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { NavSheetPanel, NavSheetTrigger, useNavSheet } from "./NavSheet";
import { PhoneSheet } from "./phone/PhoneSheet";
import { $, $$, click, render as mount } from "./phone/phone.harness";
import type { RailItem } from "./ShellRail";

const PAGE = "http://localhost:3000/page";

const mounted: Array<{ unmount(): Promise<void> }> = [];
async function render(ui: React.ReactNode) {
  const page = await mount(ui);
  mounted.push(page);
  return page;
}

const here = () => (window.history.state as { page?: string } | null)?.page;
const onDrawerEntry = () =>
  (window.history.state as { oxyPhoneSheetDepth?: number } | null)?.oxyPhoneSheetDepth === 1;
const drawer = () => $('[data-testid="nav-sheet"]');
const titles = () => $$('[data-testid="phone-sheet"] h2').map((h) => h.textContent);

async function settle() {
  await React.act(async () => {
    await new Promise((r) => setTimeout(r, 30));
  });
}

async function phoneBack() {
  await React.act(async () => {
    window.history.back();
    await new Promise((r) => setTimeout(r, 30));
  });
}

const tap = async (selector: string) => {
  await click($(selector));
  await settle();
};
const openDrawer = () => tap('[data-testid="nav-sheet-trigger"]');
const row = (id: string) => `[data-testid="nav-sheet-${id}"]`;

function Shell({ items = [], bottom }: { items?: RailItem[]; bottom?: React.ReactNode }) {
  const sheet = useNavSheet();
  return (
    <>
      <NavSheetTrigger sheet={sheet} />
      <NavSheetPanel sheet={sheet} title='Org' groups={[items]} bottom={bottom} />
    </>
  );
}

let navigations = 0;
const loads = () => navigations;
(
  globalThis as unknown as {
    jsdom: { virtualConsole: { on(event: string, fn: (e: Error) => void): void } };
  }
).jsdom.virtualConsole.on("jsdomError", (e) => {
  if (e.message.includes("navigation")) navigations += 1;
});

beforeEach(async () => {
  navigations = 0;
  // A fresh entry of our own, past whatever the last test left, so nothing of
  // theirs can answer for this one's page.
  window.history.pushState({ page: "home" }, "", PAGE);
  await new Promise((r) => setTimeout(r, 0));
});

afterEach(async () => {
  for (const page of mounted.splice(0)) await page.unmount();
  await new Promise((r) => setTimeout(r, 30));
});

describe("phone Back with the drawer open", () => {
  it("closes only the drawer, the page where it was", async () => {
    await render(<Shell />);
    const before = window.history.length;
    await openDrawer();
    expect(drawer()).not.toBeNull();
    expect(window.history.length).toBe(before + 1);
    await phoneBack();
    expect(drawer()).toBeNull();
    expect(here()).toBe("home");
    expect(window.location.href).toBe(PAGE);
  });

  it("leaves history where it was after closing by the UI", async () => {
    await render(<Shell />);
    for (const close of [".oxy-navsheet__overlay", ".oxy-navsheet__close"]) {
      await openDrawer();
      await tap(close);
      await settle();
      expect(drawer()).toBeNull();
      expect(here()).toBe("home");
    }
    // Reopening reuses the room those closes left, and Back still lands at home.
    await openDrawer();
    await phoneBack();
    expect(drawer()).toBeNull();
    expect(here()).toBe("home");
  });

  it("leaves history where it was after Escape", async () => {
    await render(<Shell />);
    await openDrawer();
    await React.act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    });
    await settle();
    expect(drawer()).toBeNull();
    expect(here()).toBe("home");
  });
});

describe("the drawer and a PhoneSheet", () => {
  function Both() {
    const sheet = useNavSheet();
    const [inner, setInner] = useState(false);
    return (
      <>
        <NavSheetTrigger sheet={sheet} />
        <button type='button' data-testid='open-sheet' onClick={() => setInner(true)}>
          sheet
        </button>
        <NavSheetPanel sheet={sheet} title='Org' groups={[[]]} />
        <PhoneSheet open={inner} onClose={() => setInner(false)} title='Inner'>
          inner
        </PhoneSheet>
      </>
    );
  }

  it("closes a sheet opened over the drawer first, then the drawer, then stays", async () => {
    await render(<Both />);
    await openDrawer();
    await tap('[data-testid="open-sheet"]');
    await phoneBack();
    expect(titles()).toEqual([]);
    expect(drawer()).not.toBeNull();
    await phoneBack();
    expect(drawer()).toBeNull();
    expect(here()).toBe("home");
  });

  it("closes a drawer opened over a sheet first, then the sheet", async () => {
    await render(<Both />);
    await tap('[data-testid="open-sheet"]');
    await openDrawer();
    await phoneBack();
    expect(drawer()).toBeNull();
    expect(titles()).toEqual(["Inner"]);
    await phoneBack();
    expect(titles()).toEqual([]);
    expect(here()).toBe("home");
  });
});

describe("choosing an item in the drawer", () => {
  const anchor: RailItem = { key: "b", label: "B", testId: "b", href: "/other-app/" };

  it("goes to an anchor's page in place of the drawer's entry", async () => {
    await render(<Shell items={[anchor]} />);
    const before = window.history.length;
    await openDrawer();
    await tap(row("b"));
    expect(drawer()).toBeNull();
    expect(loads()).toBe(1);
    // Nothing stepped down under the load: a traversal would cancel it. The entry
    // stays for the new page to replace (jsdom does not run that replace).
    expect(window.history.length).toBe(before + 1);
    expect(onDrawerEntry()).toBe(true);
    await new Promise((r) => setTimeout(r, 1100));
    expect(onDrawerEntry()).toBe(true);
  });

  it("only closes on the page already showing, with no reload and no dead entry", async () => {
    await render(<Shell items={[{ ...anchor, href: PAGE }]} />);
    await openDrawer();
    await tap(row("b"));
    await settle();
    expect(drawer()).toBeNull();
    expect(loads()).toBe(0);
    expect(here()).toBe("home");
  });

  it("leaves a modified click to the browser, and the page where it was", async () => {
    await render(<Shell items={[anchor]} />);
    await openDrawer();
    await React.act(async () => {
      $(row("b"))?.dispatchEvent(
        new MouseEvent("click", { bubbles: true, cancelable: true, metaKey: true })
      );
    });
    await settle();
    expect(drawer()).toBeNull();
    expect(here()).toBe("home");
  });

  it("leaves a click a router took to the router", async () => {
    const router = (e: Event) => e.preventDefault();
    await render(
      <div
        ref={(el) => {
          el?.addEventListener("click", router, { capture: true });
        }}
      >
        <Shell items={[anchor]} />
      </div>
    );
    await openDrawer();
    await tap(row("b"));
    expect(loads()).toBe(0);
    expect(drawer()).toBeNull();
    expect(here()).toBe("home");
  });

  it("a button that pushes navigates, and Back goes to the page it left in one step", async () => {
    const pick: RailItem = {
      key: "c",
      label: "C",
      testId: "c",
      onSelect: () => window.history.pushState({ page: "next" }, "", "/next")
    };
    await render(<Shell items={[pick]} />);
    await openDrawer();
    await tap(row("c"));
    expect(drawer()).toBeNull();
    expect(here()).toBe("next");
    expect(window.location.pathname).toBe("/next");
    await phoneBack();
    await settle();
    expect(here()).toBe("home");
    expect(window.location.href).toBe(PAGE);
  });

  it("a button that stays on the page leaves history where it was", async () => {
    const pick: RailItem = { key: "c", label: "C", testId: "c", onSelect: () => {} };
    await render(<Shell items={[pick]} />);
    await openDrawer();
    await tap(row("c"));
    await settle();
    expect(drawer()).toBeNull();
    expect(here()).toBe("home");
  });

  it("a departure that never unloads does not break the next open", async () => {
    await render(<Shell items={[anchor]} />);
    await openDrawer();
    await tap(row("b"));
    expect(drawer()).toBeNull();
    // The load came to nothing (a download, a 204): the drawer opens and closes as usual.
    await openDrawer();
    await phoneBack();
    expect(drawer()).toBeNull();
    expect(here()).toBe("home");
    await openDrawer();
    await tap(".oxy-navsheet__close");
    await settle();
    expect(here()).toBe("home");
  });

  it("wakes clean from the browser's page cache", async () => {
    await render(<Shell items={[anchor]} />);
    await openDrawer();
    await tap(row("b"));
    // Restored on the entry below, with no popstate to say so. A depth left over
    // from before would pass for the drawer's next entry, and Back would move the page.
    window.history.replaceState({ page: "restored" }, "", PAGE);
    window.dispatchEvent(new PageTransitionEvent("pageshow", { persisted: true }));
    await settle();
    await openDrawer();
    await phoneBack();
    expect(drawer()).toBeNull();
    expect(here()).toBe("restored");
  });
});
