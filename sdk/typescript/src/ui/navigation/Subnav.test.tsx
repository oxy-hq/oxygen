// @vitest-environment jsdom
//
// Subnav: a "Sections" nav of labelled links, one group per section named by its heading, the
// current section marked (its other screens too), the footer group last, a plain left click
// handed to the app's router, and in the sideways row the current link brought into the row's
// view, never the page's (widths stubbed: jsdom lays nothing out).

import { afterAll, afterEach, describe, expect, it, vi } from "vitest";
import { paintUi, unpaintUi } from "../paint.harness";
import { $, $$, fire, render } from "../ui.harness";
import { restoreLayout, stubLayout } from "./layout.harness";
import { Subnav, type SubnavProps, type SubnavSection } from "./Subnav";

const SECTIONS: SubnavSection[] = [
  {
    heading: "Records",
    items: [{ id: "stores", also: ["store", "flags"], label: "Records", href: "/" }]
  },
  { heading: "Receiving", items: [{ id: "receiving", label: "Receiving", href: "/receiving" }] },
  { heading: "Coverage", items: [{ id: "coverage", label: "Coverage", href: "/coverage" }] }
];
const FOOTER = [{ id: "gallery", label: "Gallery", href: "/gallery" }];

afterEach(() => {
  document.body.innerHTML = "";
  restoreLayout();
});

async function mount(props: Partial<SubnavProps> = {}) {
  await render(<Subnav sections={SECTIONS} footer={FOOTER} current='receiving' {...props} />);
}

const current = () => $$('a[aria-current="page"]').map((a) => a.textContent);

describe("Subnav", () => {
  it("is a 'Sections' nav, one group per section named by its heading", async () => {
    await mount();
    expect($("nav")?.getAttribute("aria-label")).toBe("Sections");
    const groups = $$('[role="group"]');
    expect(groups.map((g) => g.getAttribute("aria-label"))).toEqual([
      "Records",
      "Receiving",
      "Coverage"
    ]);
    // No printed heading: the group's name is for a screen reader only.
    expect($("nav")?.textContent).toBe("RecordsReceivingCoverageGallery");
  });

  it("marks the current section, and keeps it lit on its other screens", async () => {
    await mount();
    expect(current()).toEqual(["Receiving"]);
    document.body.innerHTML = "";
    await mount({ current: "store" });
    expect(current()).toEqual(["Records"]);
    expect($('a[aria-current="page"]')?.className).toContain("oxy-ui-section-link--on");
  });

  it("draws the footer below the divider, last, with the same links", async () => {
    await mount({ footerExtra: <button type='button'>Mode</button> });
    const footer = $(".oxy-ui-subnav__footer") as HTMLElement;
    expect(footer.parentElement?.lastElementChild).toBe(footer);
    expect([...footer.children].map((c) => c.textContent)).toEqual(["Gallery", "Mode"]);
    expect(footer.querySelector("a")?.className).toContain("oxy-ui-section-link");
    document.body.innerHTML = "";
    await render(<Subnav sections={SECTIONS} current='coverage' />);
    expect($(".oxy-ui-subnav__footer")).toBeNull();
  });

  it("hands a plain left click to onNavigate and leaves a ⌘-click to the browser", async () => {
    const onNavigate = vi.fn();
    await mount({ onNavigate });
    const coverage = $$("nav a")[2];
    const plain = new MouseEvent("click", { bubbles: true, cancelable: true });
    await fire(coverage, plain);
    expect(plain.defaultPrevented).toBe(true);
    expect(onNavigate).toHaveBeenCalledWith(
      "coverage",
      expect.objectContaining({ href: "/coverage" })
    );
    const meta = new MouseEvent("click", { bubbles: true, cancelable: true, metaKey: true });
    await fire(coverage, meta);
    expect(meta.defaultPrevented).toBe(false);
    expect(onNavigate).toHaveBeenCalledTimes(1);
  });

  it("brings the current link to the middle of a row too narrow for it, moving only the row", async () => {
    // A 200px row holding four 100px links; the current one (Coverage, the third) at 200-300.
    stubLayout({
      scrollWidth: (el) => (el.classList.contains("oxy-ui-subnav__list") ? 400 : undefined),
      clientWidth: (el) => (el.classList.contains("oxy-ui-subnav__list") ? 200 : undefined),
      rect: (el) => {
        if (el.classList.contains("oxy-ui-subnav__list")) return { left: 0, width: 200 };
        const row = el.closest(".oxy-ui-subnav__list") as HTMLElement | null;
        if (el.tagName === "A" && row) {
          const i = $$(".oxy-ui-subnav__list a").indexOf(el);
          return { left: i * 100 - row.scrollLeft, width: 100 };
        }
        return undefined;
      }
    });
    const scrollTo = vi.spyOn(window, "scrollTo").mockImplementation(() => undefined);
    await mount({ current: "coverage" });
    // Centred: its 200 left, less half of (200 row - 100 link) = 150.
    expect(($(".oxy-ui-subnav__list") as HTMLElement).scrollLeft).toBe(150);
    expect(scrollTo).not.toHaveBeenCalled();
    scrollTo.mockRestore();
  });

  it("leaves a column that fits where it is", async () => {
    stubLayout({
      scrollWidth: (el) => (el.classList.contains("oxy-ui-subnav__list") ? 200 : undefined),
      clientWidth: (el) => (el.classList.contains("oxy-ui-subnav__list") ? 200 : undefined)
    });
    await mount({ current: "coverage" });
    expect(($(".oxy-ui-subnav__list") as HTMLElement).scrollLeft).toBe(0);
  });
});

// The column's width, with ui.css painted on at 768px and up: the reference draws
// `md:w-[13.75rem]`, 220px (oxy-hq/reference-app@81ee8aa `src/chrome/Subnav.tsx:295-303`). It was
// 13rem (208px) once, from the reference's header comment, which set every page 12px wider.
describe("Subnav, painted", () => {
  afterAll(() => unpaintUi());

  it("is the reference's 220px column from 768px, and full width below", async () => {
    paintUi({ media: ["(min-width: 768px)"] });
    await mount();
    expect(getComputedStyle($(".oxy-ui-subnav") as HTMLElement).width).toBe("220px");
    paintUi();
    expect(getComputedStyle($(".oxy-ui-subnav") as HTMLElement).width).toBe("100%");
  });
});
