// @vitest-environment jsdom
//
// The wide-table scroller: a Tab stop, a named region and inset-ringed exactly while its table is
// wider than it (Store Ops' `lib/scrollFocus.test.tsx`, which this ports), the ref and the div's
// own props; and its stylesheet, read as text since jsdom lays nothing out and runs no scroll
// timeline: the end fade is invisible unless the box's sideways timeline drives it, gone at the
// end, standing off the focus ring, and pinning nothing (decision 30).

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createRef } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { ui } from "../recipes";
import { $, render } from "../ui.harness";
import { TableScroll } from "./TableScroll";

const here = dirname(fileURLToPath(import.meta.url));
const css = readFileSync(join(here, "tables.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");

afterEach(() => {
  document.body.innerHTML = "";
  delete (HTMLElement.prototype as { scrollWidth?: number }).scrollWidth;
  delete (HTMLElement.prototype as { clientWidth?: number }).clientWidth;
});

/** Report `content` px across in a `box` px box, as a browser would. */
function sizes(content: number, box: number) {
  for (const [name, value] of [
    ["scrollWidth", content],
    ["clientWidth", box]
  ] as const)
    Object.defineProperty(HTMLElement.prototype, name, { configurable: true, get: () => value });
}

const wide = (label?: string, ref?: React.Ref<HTMLDivElement>) => (
  <TableScroll label={label} ref={ref} id='box'>
    <table aria-label='Stores'>
      <tbody>
        <tr>
          <td>Walk-in cooler</td>
        </tr>
      </tbody>
    </table>
  </TableScroll>
);

/** The declarations of the rule whose selector is exactly `selector`. */
function rule(selector: string): string {
  const at = css.indexOf(`${selector} {`);
  expect(at, selector).toBeGreaterThanOrEqual(0);
  return css.slice(css.indexOf("{", at) + 1, css.indexOf("}", at));
}

describe("TableScroll", () => {
  it("is a Tab stop and a named region while its table is wider than it", async () => {
    sizes(900, 360);
    await render(wide("Stores"));
    const box = $("#box") as HTMLElement;
    expect(box.getAttribute("tabindex")).toBe("0");
    expect(box.getAttribute("role")).toBe("region");
    expect(box.getAttribute("aria-label")).toBe("Stores");
    expect(box.hasAttribute("data-scrolls")).toBe(true);
    // Its ring is drawn inside the box, which a panel clips.
    expect(box.className).toContain(ui.focusRingInset);
    expect(box.className).toContain("oxy-ui-table-scroll");
    box.focus();
    expect(document.activeElement).toBe(box);
  });

  it("is no Tab stop and no landmark while the table fits", async () => {
    sizes(360, 360);
    await render(wide("Stores"));
    const box = $("#box") as HTMLElement;
    expect(box.hasAttribute("tabindex")).toBe(false);
    expect(box.hasAttribute("role")).toBe(false);
    expect(box.hasAttribute("aria-label")).toBe(false);
    expect(box.hasAttribute("data-scrolls")).toBe(false);
  });

  it("without a label is a Tab stop with no region, and hands its ref to the box", async () => {
    sizes(900, 360);
    const ref = createRef<HTMLDivElement>();
    await render(wide(undefined, ref));
    const box = $("#box") as HTMLElement;
    expect(box.getAttribute("tabindex")).toBe("0");
    expect(box.hasAttribute("role")).toBe(false);
    expect(ref.current).toBe(box);
  });
});

describe("TableScroll's end fade (tables.css)", () => {
  it("shows nothing unless the box's own sideways scroll drives it", () => {
    const fade = rule(".oxy-ui-table-scroll::after");
    expect(fade).toMatch(/opacity:\s*0;/);
    expect(fade).toMatch(/animation:\s*oxy-ui-table-scroll-end linear both;/);
    // After the shorthand, which would reset it.
    expect(fade.indexOf("animation-timeline")).toBeGreaterThan(fade.indexOf("animation:"));
    expect(fade).toMatch(/animation-timeline:\s*scroll\(nearest inline\);/);
    expect(fade).toMatch(/pointer-events:\s*none;/);
    expect(fade).toMatch(/position:\s*sticky;/);
  });

  it("is hidden at the end of the scroll", () => {
    const frames = /@keyframes oxy-ui-table-scroll-end\s*\{([\s\S]*?)\n\}/.exec(css)?.[1] ?? "";
    expect(frames).toMatch(/100%\s*\{\s*opacity:\s*0;/);
    expect(frames).toMatch(/0%,\s*96%\s*\{\s*opacity:\s*1;/);
  });

  it("never covers the focus ring: focused, it stands off the inset ring by its width", () => {
    expect(rule(".oxy-ui-table-scroll:focus-visible::after")).toMatch(
      /margin:\s*var\(--focus-ring-width\);/
    );
  });

  it("melts into the card by default, and adds end room only while the table overflows", () => {
    const box = rule(".oxy-ui-table-scroll");
    expect(box).toMatch(/--oxy-ui-table-scroll-ground:\s*var\(--card\);/);
    expect(box).toMatch(/animation-timeline:\s*scroll\(self inline\);/);
    expect(rule(".oxy-ui-table-scroll::after")).toMatch(
      /linear-gradient\(to left, var\(--oxy-ui-table-scroll-ground\), transparent\)/
    );
  });

  it("pins no column (decision 30): no rule of its sticks anything to the left", () => {
    const scrollRules = css.split("}").filter((block) => block.includes(".oxy-ui-table-scroll"));
    expect(scrollRules.length).toBeGreaterThanOrEqual(4);
    expect(scrollRules.join("}")).not.toMatch(/(?<![\w-])left:/);
  });
});
