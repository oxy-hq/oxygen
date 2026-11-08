// @vitest-environment jsdom
//
// The phone's Back under the platform's runtime, which turns a push made before the
// person has touched the page into a replace. Not a copy of that rewrite: the block is
// read out of the real `runtime.js` and run, so these tests move when it does, and
// fail loudly if it can no longer be found. `prev` is the page before the app: landing
// on it is what leaving the app looks like here.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import * as React from "react";
import { StrictMode, useEffect, useState } from "react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { NavSheetPanel, NavSheetTrigger, useNavSheet } from "../NavSheet";
import { PhoneSheet } from "./PhoneSheet";
import { $, $$, click, render as mount } from "./phone.harness";
import { replacePageFrom } from "./sheetHistory";

const PAGE = "http://localhost:3000/page";
const RUNTIME = resolve(process.cwd(), "../../crates/api-custom-apps/src/client/runtime.js");

/** The rewrite as shipped: from its state to the listeners after the patch. */
function installRuntime() {
  const source = readFileSync(RUNTIME, "utf8");
  const from = source.indexOf("var acted = false;");
  const to = source.indexOf('window.addEventListener("popstate", onRouteChange);');
  if (from < 0 || to < from) throw new Error("runtime.js no longer has the history rewrite");
  // oxlint-disable-next-line typescript/no-implied-eval -- running our own shipped source is the point: a copy here would drift.
  new Function("onRouteChange", "onRoute", source.slice(from, to))(
    () => {},
    () => {}
  );
}

/** What any other rewriter of early pushes does: no door, every early push a replace. */
function installDoorlessRewrite() {
  const replace = window.history.replaceState;
  const pushNative = window.history.pushState;
  window.history.pushState = function (this: History, ...args: Parameters<History["pushState"]>) {
    const rewrite = !navigator.userActivation?.hasBeenActive;
    return (rewrite ? replace : pushNative).apply(this, args);
  };
}

function removeRewrite() {
  const history = window.history as unknown as Record<string, unknown>;
  delete history.pushState;
  delete history.replaceState;
}

let active = false;
Object.defineProperty(window.navigator, "userActivation", {
  configurable: true,
  get: () => ({ hasBeenActive: active })
});

/** The person touches the page. The browser's record changes before the event fires. */
async function activate() {
  active = true;
  window.dispatchEvent(new Event("pointerup"));
  await settle();
}

const mounted: Array<{ unmount(): Promise<void> }> = [];
async function render(ui: React.ReactNode) {
  const page = await mount(ui);
  mounted.push(page);
  await settle();
  return page;
}

const here = () => (window.history.state as { page?: string } | null)?.page;
const depthMark = () =>
  (window.history.state as { oxyPhoneSheetDepth?: number } | null)?.oxyPhoneSheetDepth;
const titles = () => $$('[data-testid="phone-sheet"] h2').map((h) => h.textContent);

async function settle() {
  await React.act(async () => {
    await new Promise((r) => setTimeout(r, 30));
  });
}
async function act(fn: () => void) {
  await React.act(async () => {
    fn();
    await new Promise((r) => setTimeout(r, 30));
  });
}
const phoneBack = () => act(() => window.history.back());
const tapBack = async () => {
  const backs = $$(".oxy-phone-sheet__back");
  await click(backs[backs.length - 1]);
  await settle();
};

/** Open as the page loads, which is what a URL naming a sheet does. */
function OpenAtLoad({ title = "One", onClose }: { title?: string; onClose?: () => void }) {
  const [open, setOpen] = useState(true);
  return (
    <PhoneSheet
      open={open}
      title={title}
      onClose={() => {
        onClose?.();
        setOpen(false);
      }}
    >
      <p>body</p>
    </PhoneSheet>
  );
}

function Toggled() {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button type='button' data-testid='open' onClick={() => setOpen(true)}>
        open
      </button>
      <PhoneSheet open={open} title='One' onClose={() => setOpen(false)}>
        <p>body</p>
      </PhoneSheet>
    </>
  );
}

function Nested() {
  const [top, setTop] = useState(true);
  const [under, setUnder] = useState(true);
  return (
    <>
      <PhoneSheet open={under} title='Under' onClose={() => setUnder(false)}>
        <p>under</p>
      </PhoneSheet>
      <PhoneSheet open={top} title='Top' onClose={() => setTop(false)}>
        <p>top</p>
      </PhoneSheet>
    </>
  );
}

const tapOpen = async () => {
  await click($('[data-testid="open"]'));
  await settle();
};

beforeEach(async () => {
  active = false;
  // Fresh entries of our own, past whatever the last test left.
  window.history.pushState({ page: "prev" }, "", "http://localhost:3000/prev");
  window.history.pushState({ page: "home" }, "", PAGE);
  await settle();
});

afterEach(async () => {
  for (const page of mounted.splice(0)) await page.unmount();
  await settle();
  removeRewrite();
  active = true;
  await settle();
});

describe("under the runtime: a sheet open at load", () => {
  beforeEach(installRuntime);

  it("gets a real entry, and its own Back stays on the page", async () => {
    const before = window.history.length;
    await render(<OpenAtLoad />);
    expect(window.history.length).toBe(before + 1);
    await tapBack();
    expect(titles()).toEqual([]);
    expect(here()).toBe("home");
    expect(window.location.href).toBe(PAGE);
  });

  it("closes on the phone's Back and stays on the page", async () => {
    await render(<OpenAtLoad />);
    await phoneBack();
    expect(titles()).toEqual([]);
    expect(here()).toBe("home");
  });

  it("gets one entry under StrictMode", async () => {
    const before = window.history.length;
    await render(
      <StrictMode>
        <OpenAtLoad />
      </StrictMode>
    );
    expect(window.history.length).toBe(before + 1);
    await phoneBack();
    expect(here()).toBe("home");
  });

  it("stacks nested sheets, closing one at a time on the page", async () => {
    const before = window.history.length;
    await render(<Nested />);
    expect(window.history.length).toBe(before + 2);
    await phoneBack();
    expect(titles()).toEqual(["Under"]);
    await tapBack();
    expect(titles()).toEqual([]);
    expect(here()).toBe("home");
  });

  it("still makes the app's own early push a replace, with the door shut again", async () => {
    await render(<OpenAtLoad />);
    const before = window.history.length;
    expect(document.querySelector('meta[name="oxy-history"]')).toBeNull();
    await act(() => window.history.pushState(window.history.state, "", `${PAGE}?tab=two`));
    expect(window.history.length).toBe(before);
    // It went on the sheet's entry in place, and goes down with it.
    await tapBack();
    expect(here()).toBe("home");
    expect(window.location.search).toBe("?tab=two");
  });

  it("gives a drawer open before any activation its entry too", async () => {
    function Shell() {
      const sheet = useNavSheet();
      const { setOpen } = sheet;
      useEffect(() => setOpen(true), [setOpen]);
      return (
        <>
          <NavSheetTrigger sheet={sheet} />
          <NavSheetPanel sheet={sheet} title='Org' groups={[[]]} />
        </>
      );
    }
    const before = window.history.length;
    await render(<Shell />);
    expect(window.history.length).toBe(before + 1);
    await phoneBack();
    expect($('[data-testid="nav-sheet"]')).toBeNull();
    expect(here()).toBe("home");
  });
});

describe("under the runtime: after the person has touched the page", () => {
  beforeEach(installRuntime);

  it("holds an entry through open, Back, and opening again over the one left ahead", async () => {
    await render(<Toggled />);
    await activate();
    const before = window.history.length;
    await tapOpen();
    expect(window.history.length).toBe(before + 1);
    await phoneBack();
    expect(here()).toBe("home");
    // The push lands over the entry Back left ahead, so the length holds.
    await tapOpen();
    expect(window.history.length).toBe(before + 1);
    expect(depthMark()).toBe(1);
    await tapBack();
    expect(titles()).toEqual([]);
    expect(here()).toBe("home");
  });
});

describe("a push that does not land", () => {
  beforeEach(installDoorlessRewrite);

  it("is seen: no mark left, and closing takes no step off the page", async () => {
    const before = window.history.length;
    await render(<OpenAtLoad />);
    expect(window.history.length).toBe(before);
    expect(depthMark()).toBeUndefined();
    expect(here()).toBe("home");
    await tapBack();
    expect(titles()).toEqual([]);
    expect(here()).toBe("home");
    expect(window.location.href).toBe(PAGE);
  });

  it("closes nested sheets without a step", async () => {
    await render(<Nested />);
    await tapBack();
    await tapBack();
    expect(titles()).toEqual([]);
    expect(here()).toBe("home");
  });

  it("is tried again on the first activation, and Back then closes the sheet", async () => {
    const before = window.history.length;
    await render(<OpenAtLoad />);
    await activate();
    expect(window.history.length).toBe(before + 1);
    expect(depthMark()).toBe(1);
    await phoneBack();
    expect(titles()).toEqual([]);
    expect(here()).toBe("home");
  });

  it("does not take a sheet's entry for granted after a retry that still failed", async () => {
    const before = window.history.length;
    await render(<OpenAtLoad />);
    // A touch that scrolled: an event, and still no activation.
    window.dispatchEvent(new Event("touchend"));
    await settle();
    expect(window.history.length).toBe(before);
    expect(depthMark()).toBeUndefined();
    await activate();
    expect(window.history.length).toBe(before + 1);
  });

  it("leaves a row picked with no entry to a navigation, so Back still finds the page", async () => {
    await render(<OpenAtLoad />);
    const before = window.history.length;
    // Only a fragment can load in jsdom, and it tells a replace from an assign.
    await act(() => replacePageFrom(`${PAGE}#next`));
    expect(window.history.length).toBe(before + 1);
    await phoneBack();
    expect(window.location.href).toBe(PAGE);
  });
});

describe("where the browser names its entries", () => {
  const original = Object.getOwnPropertyDescriptor(window, "navigation");
  let key = 0;
  beforeEach(() => {
    const replace = window.history.replaceState;
    const pushNative = window.history.pushState;
    Object.defineProperty(window, "navigation", {
      configurable: true,
      value: {
        currentEntry: {
          get key() {
            return String(key);
          }
        }
      }
    });
    // A rewrite with a door: the push lands, and is the only thing that moves the key.
    window.history.pushState = function (this: History, ...args: Parameters<History["pushState"]>) {
      const allowed = active || document.querySelector('meta[name="oxy-history"]');
      if (allowed) key += 1;
      return (allowed ? pushNative : replace).apply(this, args);
    };
  });
  afterEach(() => {
    if (original) Object.defineProperty(window, "navigation", original);
    else delete (window as unknown as { navigation?: unknown }).navigation;
  });

  it("believes the key over a length that held", async () => {
    // One entry ahead, not yet activated: a push truncates it and the length holds.
    active = true;
    await act(() => window.history.pushState({ page: "ahead" }, "", `${PAGE}#ahead`));
    await act(() => window.history.back());
    active = false;
    expect(here()).toBe("home");
    const before = window.history.length;
    await render(<OpenAtLoad />);
    expect(window.history.length).toBe(before);
    expect(depthMark()).toBe(1);
    await tapBack();
    expect(here()).toBe("home");
  });
});

describe.each([
  ["without the runtime", () => {}],
  ["under the runtime", installRuntime]
])("an entry left by an earlier load, %s", (_name, install) => {
  beforeEach(install);

  const leftBy = (run: string, depth = 1) =>
    window.history.pushState(
      { page: "left", oxyPhoneSheetDepth: depth, oxyPhoneSheetRun: run },
      "",
      window.location.href
    );

  // The real runtime would turn these setup pushes into replaces.
  const atLeftEntry = async (run: string, depth = 1) => {
    active = true;
    leftBy(run, depth);
    await settle();
    active = false;
  };

  it("is taken back by the sheet opening on it, so a reload adds no entry", async () => {
    await atLeftEntry("c1-earlier");
    const before = window.history.length;
    await render(<OpenAtLoad />);
    expect(window.history.length).toBe(before);
    expect(here()).toBe("left");
    await phoneBack();
    expect(titles()).toEqual([]);
    expect(here()).toBe("home");
  });

  it("goes down with the URL the page has when the sheet closes", async () => {
    await atLeftEntry("c1-earlier");
    await render(
      <OpenAtLoad
        onClose={() => window.history.replaceState(window.history.state, "", `${PAGE}?closed`)}
      />
    );
    await tapBack();
    expect(here()).toBe("home");
    expect(window.location.search).toBe("?closed");
  });

  it("steps down to match when fewer sheets came back than it had", async () => {
    await atLeftEntry("c1-earlier");
    active = true;
    window.history.pushState(
      { page: "left2", oxyPhoneSheetDepth: 2, oxyPhoneSheetRun: "c1-earlier" },
      "",
      window.location.href
    );
    active = false;
    await render(<OpenAtLoad />);
    expect(here()).toBe("left");
    await tapBack();
    expect(here()).toBe("home");
  });

  it("closes only the top sheet on the phone's Back when two came back", async () => {
    await atLeftEntry("c1-earlier");
    active = true;
    window.history.pushState(
      { page: "left2", oxyPhoneSheetDepth: 2, oxyPhoneSheetRun: "c1-earlier" },
      "",
      window.location.href
    );
    active = false;
    const before = window.history.length;
    await render(<Nested />);
    expect(window.history.length).toBe(before);
    await phoneBack();
    expect(titles()).toEqual(["Under"]);
    expect(here()).toBe("left");
    await phoneBack();
    expect(titles()).toEqual([]);
    expect(here()).toBe("home");
  });

  it("is not trusted from a kit that assumed its pushes landed", async () => {
    await atLeftEntry("mw4y2cn7-n44q36ldlj");
    const before = window.history.length;
    await render(<OpenAtLoad />);
    expect(window.history.length).toBe(before + 1);
    await tapBack();
    expect(here()).toBe("left");
  });

  it("carries the page's URL down even when the sheet changed nothing", async () => {
    // A filter was in the URL when the page reloaded; the entry under it never saw it.
    await act(() => window.history.replaceState({ page: "home" }, "", PAGE));
    active = true;
    window.history.pushState(
      { page: "left", oxyPhoneSheetDepth: 1, oxyPhoneSheetRun: "c1-earlier" },
      "",
      `${PAGE}?filter=on`
    );
    active = false;
    await render(<OpenAtLoad />);
    await tapBack();
    expect(here()).toBe("home");
    expect(window.location.search).toBe("?filter=on");
  });
});
