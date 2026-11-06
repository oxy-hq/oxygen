// @vitest-environment jsdom
//
// PhonePage scrolls with the scroller around it, so switching tabs must put
// that scroller (the shell's main area, else the document) back at the top —
// and only when the key changes, not on every render.

import { afterEach, describe, expect, it } from "vitest";
import { PhonePage } from "./PhonePage";
import { PhoneStickyAction } from "./PhoneStickyAction";
import { PhoneTabBar } from "./PhoneTabBar";
import { $, render } from "./phone.harness";

afterEach(() => {
  document.body.innerHTML = "";
});

/** A scroller like `.oxy-shell__main`, recording what is written to it. */
function scroller(): { el: HTMLDivElement; writes: number[] } {
  const el = document.createElement("div");
  el.className = "oxy-shell__main";
  el.style.overflowY = "auto";
  document.body.appendChild(el);
  const writes: number[] = [];
  let top = 480;
  // jsdom lays nothing out, so say that this box really has more than it shows.
  Object.defineProperty(el, "scrollHeight", { configurable: true, get: () => 2000 });
  Object.defineProperty(el, "clientHeight", { configurable: true, get: () => 800 });
  Object.defineProperty(el, "scrollTop", {
    configurable: true,
    get: () => top,
    set: (v: number) => {
      top = v;
      writes.push(v);
    }
  });
  return { el, writes };
}

const tabs = [
  { id: "overview", label: "Overview" },
  { id: "stores", label: "Stores" }
];

describe("PhonePage", () => {
  it("scrolls the nearest scroller to the top when scrollKey changes", async () => {
    const { el, writes } = scroller();
    const page = await render(<PhonePage scrollKey='overview'>body</PhonePage>, el);
    expect(writes).toEqual([]); // not on mount
    await page.rerender(<PhonePage scrollKey='overview'>body, again</PhonePage>);
    expect(writes).toEqual([]); // not on a re-render with the same key
    await page.rerender(<PhonePage scrollKey='stores'>stores</PhonePage>);
    expect(writes).toEqual([0]);
    expect(el.scrollTop).toBe(0);
    await page.unmount();
  });

  it("falls back to the document when nothing around it scrolls", async () => {
    const doc = (document.scrollingElement as HTMLElement | null) ?? document.documentElement;
    const writes: number[] = [];
    Object.defineProperty(doc, "scrollTop", {
      configurable: true,
      get: () => 300,
      set: (v: number) => writes.push(v)
    });
    const page = await render(<PhonePage scrollKey={1}>a</PhonePage>);
    await page.rerender(<PhonePage scrollKey={2}>b</PhonePage>);
    expect(writes).toEqual([0]);
    await page.unmount();
    Reflect.deleteProperty(doc, "scrollTop");
  });

  it("skips a box that is only overflow-marked, and the body whose overflow is the viewport's", async () => {
    // `overflow-x: hidden` computes `overflow-y` to auto on a box that never
    // scrolls; writing scrollTop to it does nothing and the page stays put.
    const doc = (document.scrollingElement as HTMLElement | null) ?? document.documentElement;
    const writes: number[] = [];
    Object.defineProperty(doc, "scrollTop", {
      configurable: true,
      get: () => 300,
      set: (v: number) => writes.push(v)
    });
    document.body.style.overflowX = "hidden";
    document.body.style.overflowY = "auto";
    Object.defineProperty(document.body, "scrollHeight", { configurable: true, get: () => 2000 });
    Object.defineProperty(document.body, "clientHeight", { configurable: true, get: () => 800 });
    const wrapper = document.createElement("div");
    wrapper.style.overflowY = "auto";
    document.body.appendChild(wrapper);
    const page = await render(<PhonePage scrollKey={1}>a</PhonePage>, wrapper);
    await page.rerender(<PhonePage scrollKey={2}>b</PhonePage>);
    expect(writes).toEqual([0]);
    await page.unmount();
    Reflect.deleteProperty(doc, "scrollTop");
    Reflect.deleteProperty(document.body, "scrollHeight");
    Reflect.deleteProperty(document.body, "clientHeight");
    document.body.removeAttribute("style");
  });

  it("grows to the scroller's bottom through a wrapper that hugs its content", async () => {
    // `min-height: 100%` resolves to nothing under an auto-height wrapper, so
    // a short page left its dock mid-screen. jsdom has no layout: state it.
    const main = document.createElement("div");
    main.style.overflowY = "auto";
    document.body.appendChild(main);
    const wrapper = document.createElement("div");
    main.appendChild(wrapper);
    const box = (el: Element, top: number, height: number) => {
      el.getBoundingClientRect = () =>
        ({
          top,
          bottom: top + height,
          height,
          left: 0,
          right: 0,
          width: 0,
          x: 0,
          y: top
        }) as DOMRect;
    };
    box(main, 49, 795);
    Object.defineProperty(main, "clientHeight", { configurable: true, get: () => 795 });
    Object.defineProperty(main, "scrollHeight", { configurable: true, get: () => 795 });
    box(wrapper, 49, 135);
    const proto = HTMLDivElement.prototype;
    const real = proto.getBoundingClientRect;
    const pageBox = { top: 49, bottom: 184, height: 135 } as DOMRect;
    proto.getBoundingClientRect = () => {
      return pageBox; // the page, and the harness's own div around it
    };
    const page = await render(
      <PhonePage tabBar={<PhoneTabBar items={tabs} activeId='overview' onSelect={() => {}} />}>
        short
      </PhonePage>,
      wrapper
    );
    expect(($('[data-testid="phone-page"]') as HTMLElement).style.minHeight).toBe("795px");
    proto.getBoundingClientRect = real;
    await page.unmount();
  });

  it("docks the sticky action above the tab bar", async () => {
    const page = await render(
      <PhonePage
        stickyAction={
          <PhoneStickyAction>
            <button type='button'>Post</button>
          </PhoneStickyAction>
        }
        tabBar={<PhoneTabBar items={tabs} activeId='overview' onSelect={() => {}} />}
      >
        body
      </PhonePage>
    );
    const dock = $(".oxy-phone-page__dock") as HTMLElement;
    expect([...dock.children].map((c) => c.classList[1])).toEqual([
      "oxy-phone-sticky",
      "oxy-phone-tabbar"
    ]);
    // The dock comes after the body, so the content flows above it.
    expect($(".oxy-phone-page__body")?.nextElementSibling).toBe(dock);
    expect($(".oxy-phone-page")?.classList.contains("oxy-phone-page--tabbar")).toBe(true);
    await page.unmount();
  });

  it("has no dock, and clears the home indicator itself, when nothing is docked", async () => {
    const page = await render(<PhonePage>body</PhonePage>);
    expect($(".oxy-phone-page__dock")).toBeNull();
    expect($(".oxy-phone-page")?.classList.contains("oxy-phone-page--bare")).toBe(true);
    // The type and touch floor applies inside the page.
    expect($(".oxy-phone-page")?.classList.contains("oxy-phone")).toBe(true);
    await page.unmount();
  });
});
