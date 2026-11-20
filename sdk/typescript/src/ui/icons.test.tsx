// @vitest-environment jsdom
//
// The desk UI's glyphs are Phosphor regular, the design system's set, drawn inline so the SDK
// carries no icon dependency; decorative, and sized by class.

import { describe, expect, it } from "vitest";
import * as icons from "./icons";
import { ui } from "./recipes";
import { render } from "./ui.harness";

describe("ui icons", () => {
  const all = Object.entries(icons).filter(([name]) => name.endsWith("Icon"));

  it("are Phosphor: a 256 viewBox, the text colour, no stroke, hidden from assistive tech", async () => {
    expect(all.length).toBeGreaterThanOrEqual(7);
    for (const [name, Icon] of all) {
      const G = Icon as (p: icons.IconProps) => React.ReactNode;
      const page = await render(<G />);
      const svg = page.container.querySelector("svg") as SVGElement;
      expect(svg.getAttribute("viewBox"), name).toBe("0 0 256 256");
      expect(svg.getAttribute("fill"), name).toBe("currentColor");
      expect(svg.getAttribute("stroke"), name).toBeNull();
      expect(svg.getAttribute("aria-hidden"), name).toBe("true");
      expect(svg.querySelectorAll("path").length, name).toBeGreaterThan(0);
      await page.unmount();
    }
  });

  it("take the control size by default and the inline size when asked", async () => {
    const page = await render(
      <>
        <icons.PlusIcon />
        <icons.PlusIcon className={ui.iconInline} />
      </>
    );
    const [control, inline] = [...page.container.querySelectorAll("svg")];
    expect(control.getAttribute("class")).toBe(`oxy-ui-icon ${ui.iconControl}`);
    expect(inline.getAttribute("class")).toBe(`oxy-ui-icon ${ui.iconInline}`);
  });
});
