// @vitest-environment jsdom
//
// 2.32.0 Subnav `showHeadings` and `header` (decision 99, Quadrogen's lab names over their modules
// and its plant line): printed in the eyebrow, the heading hidden from a screen reader that
// already hears it as the group's name, the header drawn in the desk column only, the column at
// the SDK's own width. Off by default (keptPieces.defaults.test.tsx).

import { afterAll, afterEach, describe, expect, it } from "vitest";
import { paintUi, unpaintUi } from "../paint.harness";
import { ui } from "../recipes";
import { $, $$, render } from "../ui.harness";
import { Subnav, type SubnavProps, type SubnavSection } from "./Subnav";

const SECTIONS: SubnavSection[] = [
  { heading: "Fleet", items: [{ id: "fleet/overview", label: "Overview", href: "#/fleet" }] },
  {
    heading: "Operations",
    items: [
      { id: "ops/plant", label: "Plant", href: "#/ops/plant" },
      { id: "ops/cause", label: "Cause", href: "#/ops/cause" }
    ]
  }
];

afterEach(() => {
  document.body.innerHTML = "";
});
afterAll(() => unpaintUi());

async function mount(props: Partial<SubnavProps> = {}) {
  await render(<Subnav sections={SECTIONS} current='ops/plant' {...props} />);
}

describe("Subnav showHeadings and header", () => {
  it("prints each section's heading over its links, in the eyebrow, hidden from a screen reader", async () => {
    await mount({ showHeadings: true });
    const headings = $$(".oxy-ui-subnav__heading");
    expect(headings.map((h) => h.textContent)).toEqual(["Fleet", "Operations"]);
    for (const h of headings) {
      expect(h.getAttribute("aria-hidden")).toBe("true");
      expect(h.className).toContain(ui.eyebrow);
    }
    // Each heading leads its own group, which keeps its name.
    const groups = $$('[role="group"]');
    expect(groups.map((g) => g.firstElementChild?.textContent)).toEqual(["Fleet", "Operations"]);
    expect(groups.map((g) => g.getAttribute("aria-label"))).toEqual(["Fleet", "Operations"]);
  });

  it("puts the header first in the column, in the eyebrow, read as text", async () => {
    await mount({ header: "P-001 Plant 01" });
    const header = $(".oxy-ui-subnav__header");
    expect(header?.textContent).toBe("P-001 Plant 01");
    expect(header?.className).toContain(ui.eyebrow);
    expect(header?.getAttribute("aria-hidden")).toBeNull();
    expect($(".oxy-ui-subnav__list")?.firstElementChild).toBe(header);
    expect($$(".oxy-ui-subnav__heading")).toHaveLength(0);
  });

  it("draws the header in the desk column only, and keeps the column's own width", async () => {
    await mount({ header: "P-001 Plant 01", showHeadings: true });
    paintUi();
    expect(getComputedStyle($(".oxy-ui-subnav__header") as HTMLElement).display).toBe("none");
    paintUi({ media: ["(min-width: 768px)"] });
    expect(getComputedStyle($(".oxy-ui-subnav__header") as HTMLElement).display).toBe("block");
    expect(getComputedStyle($(".oxy-ui-subnav") as HTMLElement).width).toBe("220px");
    const heading = getComputedStyle($(".oxy-ui-subnav__heading") as HTMLElement);
    expect(heading.paddingLeft).toBe("10px");
    expect(heading.marginTop).toBe("8px");
    expect(heading.marginBottom).toBe("4px");
  });
});
