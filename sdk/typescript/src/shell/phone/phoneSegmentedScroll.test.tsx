// @vitest-environment jsdom
//
// PhoneSegmented's row when it is wider than the screen: which edges fade, and
// the selected option kept wholly in view, clear of the fades. jsdom lays nothing
// out, so the row's geometry is stubbed: a 200px window on a 300px row of three
// 100px options. The layout itself (columns by their labels, the scroll, the mask)
// was measured in Chrome for the PR, on Locations' four-option site sheet at 320.

import { afterEach, describe, expect, it, vi } from "vitest";
import { PhoneSegmented } from "./PhoneSegmented";
import { $, render } from "./phone.harness";
import { edgesToFade, SEGMENT_FADE_PX, scrollToShow } from "./segmentedScroll";

const OPTIONS = [
  { value: "a", label: "Attention" },
  { value: "b", label: "Books" },
  { value: "c", label: "All" }
];

/** A row `width` wide showing `view` of it from `left`, its options `size` each. */
function stubRow(width: number, view: number, size = 100) {
  const at = { left: 0 };
  const isRow = (el: Element) => el.classList.contains("oxy-phone-segmented");
  const box = (x: number, w: number) => ({ left: x, right: x + w, width: w }) as DOMRect;
  const restore = [
    vi.spyOn(HTMLElement.prototype, "scrollWidth", "get").mockImplementation(function (
      this: HTMLElement
    ) {
      return isRow(this) ? width : 0;
    }),
    vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockImplementation(function (
      this: HTMLElement
    ) {
      return isRow(this) ? view : 0;
    }),
    vi.spyOn(HTMLElement.prototype, "scrollLeft", "get").mockImplementation(() => at.left),
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (
      this: HTMLElement
    ) {
      if (isRow(this)) return box(0, view);
      const i = [...(this.parentElement?.children ?? [])].indexOf(this);
      return box(i * size - at.left, size);
    })
  ];
  const scrollTo = vi.fn((o: ScrollToOptions) => {
    at.left = o.left ?? at.left;
  });
  (HTMLElement.prototype as { scrollTo: unknown }).scrollTo = scrollTo;
  return {
    at,
    scrollTo,
    restore: () => {
      for (const r of restore) r.mockRestore();
      delete (HTMLElement.prototype as { scrollTo?: unknown }).scrollTo;
    }
  };
}

let stub: ReturnType<typeof stubRow> | null = null;
afterEach(() => {
  stub?.restore();
  stub = null;
  document.body.innerHTML = "";
});

const row = () => $(".oxy-phone-segmented") as HTMLElement;

describe("PhoneSegmented, a row wider than the screen", () => {
  it("fades the edges with more beyond them, and none when the row fits", () => {
    const el = (scrollWidth: number, clientWidth: number, scrollLeft: number) =>
      ({ scrollWidth, clientWidth, scrollLeft }) as HTMLElement;
    expect(edgesToFade(el(288, 288, 0))).toBe("");
    // A fraction of a pixel over is a row that fits.
    expect(edgesToFade(el(288.6, 288, 0))).toBe("");
    expect(edgesToFade(el(300, 200, 0))).toBe("end");
    expect(edgesToFade(el(300, 200, 50))).toBe("start end");
    expect(edgesToFade(el(300, 200, 100))).toBe("start");
  });

  it("scrolls the selected option into view at once as it first draws, clear of the end's fade", async () => {
    stub = stubRow(300, 200);
    const page = await render(<PhoneSegmented options={OPTIONS} value='c' onChange={() => {}} />);
    // The last option, 200-300: the row scrolls to its end, where no fade is drawn.
    expect(stub.scrollTo).toHaveBeenLastCalledWith({ left: 100, behavior: "auto" });
    await page.unmount();
  });

  it("marks the faded edges, again on each scroll", async () => {
    stub = stubRow(300, 200);
    const page = await render(<PhoneSegmented options={OPTIONS} value='a' onChange={() => {}} />);
    expect(stub.scrollTo).not.toHaveBeenCalled();
    expect(row().getAttribute("data-oxy-fade")).toBe("end");
    stub.at.left = 50;
    row().dispatchEvent(new Event("scroll"));
    expect(row().getAttribute("data-oxy-fade")).toBe("start end");
    stub.at.left = 100;
    row().dispatchEvent(new Event("scroll"));
    expect(row().getAttribute("data-oxy-fade")).toBe("start");
    await page.unmount();
  });

  it("moves smoothly to a new choice that sits under a fade", async () => {
    stub = stubRow(300, 200);
    const page = await render(<PhoneSegmented options={OPTIONS} value='a' onChange={() => {}} />);
    // "Books" at 100-200 in a 0-200 window: under the end's 24px fade.
    await page.rerender(<PhoneSegmented options={OPTIONS} value='b' onChange={() => {}} />);
    expect(stub.scrollTo).toHaveBeenLastCalledWith({
      left: SEGMENT_FADE_PX,
      behavior: "smooth"
    });
    await page.unmount();
  });

  it("leaves a row that fits alone: no fade, no scroll", async () => {
    stub = stubRow(288, 288, 96);
    const page = await render(<PhoneSegmented options={OPTIONS} value='c' onChange={() => {}} />);
    expect(row().hasAttribute("data-oxy-fade")).toBe(false);
    expect(stub.scrollTo).not.toHaveBeenCalled();
    await page.unmount();
  });

  it("puts an option that is already clear of the fades nowhere new", () => {
    const rowEl = {
      scrollWidth: 300,
      clientWidth: 200,
      scrollLeft: 50,
      getBoundingClientRect: () => ({ left: 0, right: 200 })
    } as unknown as HTMLElement;
    const option = (left: number, right: number) =>
      ({ getBoundingClientRect: () => ({ left, right }) }) as unknown as HTMLElement;
    expect(scrollToShow(rowEl, option(50, 150))).toBeNull();
    // Under the start's fade: back by what it takes to clear it, never before the row's start.
    expect(scrollToShow(rowEl, option(10, 110))).toBe(36);
    expect(scrollToShow(rowEl, option(-50, 50))).toBe(0);
  });
});
