// @vitest-environment jsdom
//
// The kit's glyphs are Phosphor regular, the design system's set, drawn inline
// so the SDK carries no icon dependency.

import { describe, expect, it } from "vitest";
import * as icons from "./icons";
import { render } from "./phone.harness";

describe("phone kit icons", () => {
  it("are Phosphor: a 256 viewBox, filled with the text colour, no stroke", async () => {
    const all = Object.entries(icons).filter(([name]) => name.endsWith("Icon"));
    expect(all.length).toBeGreaterThanOrEqual(6);
    for (const [name, Icon] of all) {
      const G = Icon as (p: { size?: number }) => React.ReactNode;
      const page = await render(<G />);
      const svg = page.container.querySelector("svg") as SVGElement;
      expect(svg.getAttribute("viewBox"), name).toBe("0 0 256 256");
      expect(svg.getAttribute("fill"), name).toBe("currentColor");
      expect(svg.getAttribute("stroke"), name).toBeNull();
      await page.unmount();
    }
  });
});
