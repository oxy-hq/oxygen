// @vitest-environment jsdom
//
// Closing a PhoneSheet puts the page behind exactly where it was. The lock
// alone left that to the browser, which keeps a scroller's position only while
// its content is tall enough: a list that rendered shorter under the sheet came
// back at the top, and a Back landed on whatever the browser had saved for the
// page's entry, the clamped top again. jsdom lays nothing out, so the scroller
// here is a box that clamps its scrollTop to the height the test gives it.

import { useState } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { PhonePage } from "./PhonePage";
import { PhoneSheet } from "./PhoneSheet";
import { $, click, render } from "./phone.harness";

const VIEW = 800;

/** A `.oxy-shell__main` whose content height the test sets, clamping as a browser does. */
function scroller(height = 4000) {
  const el = document.createElement("div");
  el.className = "oxy-shell__main";
  // Both: jsdom does not read the shorthand back as overflow-y, which PhonePage
  // looks at to find its scroller.
  el.style.overflow = "auto";
  el.style.overflowY = "auto";
  document.body.appendChild(el);
  const box = { height, top: 0 };
  const max = () => Math.max(0, box.height - VIEW);
  Object.defineProperty(el, "scrollHeight", { configurable: true, get: () => box.height });
  Object.defineProperty(el, "clientHeight", { configurable: true, get: () => VIEW });
  Object.defineProperty(el, "scrollTop", {
    configurable: true,
    get: () => box.top,
    set: (v: number) => {
      box.top = Math.min(Math.max(0, v), max());
    }
  });
  return {
    el,
    /** A person scrolling: the position, and the scroll event a browser fires for it. */
    scrollTo(top: number) {
      el.scrollTop = top;
      el.dispatchEvent(new Event("scroll"));
    },
    /** The content changing height; a shorter one clamps the position, as layout does. */
    resize(next: number) {
      box.height = next;
      box.top = Math.min(box.top, max());
    }
  };
}

const frames = (n = 2) =>
  new Promise<void>((done) => {
    const tick = (left: number) =>
      left === 0 ? done() : requestAnimationFrame(() => tick(left - 1));
    tick(n);
  });

let shrinkOnOpen: (() => void) | null = null;

function Page({ scrollKey }: { scrollKey?: string }) {
  const [open, setOpen] = useState(false);
  return (
    <PhonePage scrollKey={scrollKey}>
      <button
        type='button'
        data-testid='open'
        onClick={() => {
          shrinkOnOpen?.();
          setOpen(true);
        }}
      >
        Almaden
      </button>
      <PhoneSheet open={open} onClose={() => setOpen(false)} title='Almaden'>
        detail
      </PhoneSheet>
    </PhonePage>
  );
}

/** A page whose sheet picks a tab, like the tab bar's More list. */
function Tabs() {
  const [open, setOpen] = useState(false);
  const [tab, setTab] = useState("stores");
  return (
    <PhonePage scrollKey={tab}>
      <button type='button' data-testid='open' onClick={() => setOpen(true)}>
        More
      </button>
      <PhoneSheet open={open} onClose={() => setOpen(false)} title='More'>
        <button
          type='button'
          data-testid='pick'
          onClick={() => {
            setTab("flags");
            setOpen(false);
          }}
        >
          Flags
        </button>
      </PhoneSheet>
    </PhonePage>
  );
}

afterEach(() => {
  shrinkOnOpen = null;
  document.body.innerHTML = "";
  document.documentElement.style.overflow = "";
  window.history.replaceState(null, "", "/");
});

describe("PhoneSheet keeps the page's scroll position", () => {
  it("puts back a position the list lost while it was shorter under the sheet", async () => {
    const box = scroller();
    const page = await render(<Page />, box.el);
    box.scrollTo(1500);
    await click($('[data-testid="open"]'));
    box.resize(300); // a loading state: clamped to the top
    box.resize(4000); // the rows are back, the position is not
    expect(box.el.scrollTop).toBe(0);
    await click($(".oxy-phone-sheet__back"));
    expect(box.el.scrollTop).toBe(1500);
    await page.unmount();
  });

  it("puts back a position clamped by the very commit that opened the sheet", async () => {
    const box = scroller();
    const page = await render(<Page />, box.el);
    box.scrollTo(1500);
    // Clamped before the sheet's lock runs, and before any scroll event says so.
    shrinkOnOpen = () => box.resize(300);
    await click($('[data-testid="open"]'));
    box.resize(4000);
    await click($(".oxy-phone-sheet__back"));
    expect(box.el.scrollTop).toBe(1500);
    await page.unmount();
  });

  it("holds the document while a Back lands and the browser restores what it saved", async () => {
    const doc = document.documentElement;
    const box = { top: 0 };
    Object.defineProperty(doc, "scrollHeight", { configurable: true, get: () => 4000 });
    Object.defineProperty(doc, "clientHeight", { configurable: true, get: () => VIEW });
    Object.defineProperty(doc, "scrollTop", {
      configurable: true,
      get: () => box.top,
      set: (v: number) => {
        box.top = v;
      }
    });
    try {
      const page = await render(<Page />);
      doc.scrollTop = 1500;
      document.dispatchEvent(new Event("scroll"));
      await click($('[data-testid="open"]'));
      await click($(".oxy-phone-sheet__back"));
      // The traversal lands after the sheet has gone, and the browser restores what it saved.
      window.dispatchEvent(new PopStateEvent("popstate"));
      doc.scrollTop = 0;
      await frames();
      expect(doc.scrollTop).toBe(1500);
      await page.unmount();
    } finally {
      for (const key of ["scrollHeight", "clientHeight", "scrollTop"])
        Reflect.deleteProperty(doc, key);
    }
  });

  it("holds it while the list is still too short, and puts it back when the rows arrive", async () => {
    const box = scroller();
    const page = await render(<Page />, box.el);
    box.scrollTo(1500);
    await click($('[data-testid="open"]'));
    box.resize(300);
    await click($(".oxy-phone-sheet__back"));
    expect(box.el.scrollTop).toBe(0);
    await frames();
    box.resize(4000); // the rows render a few frames after the sheet goes
    await frames();
    expect(box.el.scrollTop).toBe(1500);
    await page.unmount();
  });

  it("lets the app's own scroll right after the close stand", async () => {
    const box = scroller();
    const page = await render(<Page />, box.el);
    box.scrollTo(1500);
    await click($('[data-testid="open"]'));
    await click($(".oxy-phone-sheet__back"));
    box.el.scrollTop = 300; // a saved row brought into view, in the same tick
    await frames(3);
    expect(box.el.scrollTop).toBe(300);
    await page.unmount();
  });

  it("lets go as soon as the person touches the page", async () => {
    const box = scroller();
    const page = await render(<Page />, box.el);
    box.scrollTo(1500);
    await click($('[data-testid="open"]'));
    await click($(".oxy-phone-sheet__back"));
    document.dispatchEvent(new Event("pointerdown"));
    box.scrollTo(300);
    await frames();
    expect(box.el.scrollTop).toBe(300);
    await page.unmount();
  });

  it("leaves a page that a link inside the sheet went to where it starts", async () => {
    const box = scroller();
    const page = await render(<Page />, box.el);
    box.scrollTo(1500);
    await click($('[data-testid="open"]'));
    window.history.pushState(null, "", "/stores/almaden");
    box.el.scrollTop = 0;
    await click($(".oxy-phone-sheet__back"));
    await frames();
    expect(box.el.scrollTop).toBe(0);
    await page.unmount();
  });

  it("lets a new tab chosen as the sheet closes start at its top", async () => {
    const box = scroller();
    const page = await render(<Tabs />, box.el);
    box.scrollTo(1500);
    await click($('[data-testid="open"]'));
    // One commit: the sheet closes and the page's key changes, as the More list does.
    await click($('[data-testid="pick"]'));
    await frames();
    expect(box.el.scrollTop).toBe(0);
    await page.unmount();
  });
});
