// @vitest-environment jsdom
//
// AppMark's promises (the reference's `src/lib/appMark.test.tsx`, driven on a real mount): every
// mark points its glyph at a gradient of its own, so two marks on one screen never share or
// shadow an id; the fade stops at the floor that keeps the glyph's foot at 3:1 (measured in
// contrast.cases.ts); the three sizes are three squares; the mark is hidden from assistive tech
// and the name beside it is what is read; the glyph is handed the fill weight and the gradient.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vitest";
import { $, $$, render } from "../ui.harness";
import { APP_MARK_FADE_FLOOR, AppMark, type AppMarkGlyphProps } from "./AppMark";

afterEach(() => {
  document.body.innerHTML = "";
});

const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "figures.css"), "utf8");
const rule = (selector: string) => {
  const at = css.indexOf(`${selector} {`);
  return at < 0 ? "" : css.slice(at, css.indexOf("}", at));
};

/** A glyph of the shape Phosphor's icons have (`weight`, `color`, `size`, `children`), as an app
 *  without Phosphor would write one. It records what it was handed. */
const handed: AppMarkGlyphProps[] = [];
function Box(props: AppMarkGlyphProps) {
  handed.push(props);
  const { color, size, className, children, weight } = props;
  return (
    <svg
      viewBox='0 0 256 256'
      fill={color}
      width={size}
      height={size}
      className={className}
      aria-hidden='true'
      data-weight={weight}
    >
      {children}
      <rect x='32' y='32' width='192' height='192' />
    </svg>
  );
}

describe("AppMark", () => {
  it("gives each mark its own gradient, and fills the glyph from it", async () => {
    await render(
      <>
        <AppMark glyph={Box} />
        <AppMark glyph={Box} size='sm' />
        <AppMark glyph={Box} size='lg' />
      </>
    );
    const ids = $$("linearGradient").map((g) => g.id);
    expect(ids).toHaveLength(3);
    expect(new Set(ids).size).toBe(3);
    const glyphs = $$("[data-app-mark] svg");
    for (const [i, id] of ids.entries()) {
      expect(id).toMatch(/^app-mark-[A-Za-z0-9_-]+$/);
      // Each glyph's fill names its own gradient, and that gradient sits inside the same svg.
      expect(glyphs[i].getAttribute("fill")).toBe(`url(#${id})`);
      expect(glyphs[i].querySelector(`linearGradient#${id}`)).not.toBeNull();
    }
  });

  it("fades the ink from full at the glyph's top to the floor at its foot, in the 256 box", async () => {
    expect(APP_MARK_FADE_FLOOR).toBeGreaterThanOrEqual(0.7);
    await render(<AppMark glyph={Box} />);
    const g = $("linearGradient") as Element;
    expect(g.getAttribute("gradientUnits")).toBe("userSpaceOnUse");
    expect([g.getAttribute("y1"), g.getAttribute("y2")]).toEqual(["0", "256"]);
    const stops = [...g.querySelectorAll("stop")] as SVGStopElement[];
    expect(stops.map((s) => s.getAttribute("offset"))).toEqual(["0", "1"]);
    expect(stops[0].style.stopColor).toBe("var(--app-mark-ink)");
    expect(stops[0].style.stopOpacity).toBe("");
    expect(stops[1].style.stopColor).toBe("var(--app-mark-ink)");
    expect(Number(stops[1].style.stopOpacity)).toBe(APP_MARK_FADE_FLOOR);
  });

  it("hands the glyph the fill weight, its gradient, 56% of the square, and hides it", async () => {
    handed.length = 0;
    await render(<AppMark glyph={Box} />);
    const last = handed[handed.length - 1];
    expect(last.weight).toBe("fill");
    expect(last.color).toMatch(/^url\(#app-mark-/);
    expect(last.size).toBe("56%");
    expect(last.className).toBe("oxy-ui-app-mark__glyph");
    expect(last["aria-hidden"]).toBe("true");
    expect(rule(".oxy-ui-app-mark__glyph")).toMatch(/width: 56%;\s*height: 56%;/);
  });

  it("draws three squares, 32, 40 and 56px, on the ground token; md by default", async () => {
    const page = await render(<AppMark glyph={Box} />);
    expect(($("[data-app-mark]") as HTMLElement).classList).toContain("oxy-ui-app-mark--md");
    await page.unmount();
    for (const [size, px] of [
      ["sm", 32],
      ["md", 40],
      ["lg", 56]
    ] as const) {
      const one = await render(<AppMark glyph={Box} size={size} />);
      const mark = $("[data-app-mark]") as HTMLElement;
      expect(mark.classList, size).toContain(`oxy-ui-app-mark--${size}`);
      expect(rule(`.oxy-ui-app-mark--${size}`), size).toMatch(
        new RegExp(`width: ${px}px;\\s*height: ${px}px;`)
      );
      await one.unmount();
    }
    expect(rule(".oxy-ui-app-mark")).toMatch(/background-color: var\(--app-mark-ground\);/);
  });

  it("is hidden from assistive tech: the app's name beside it is what is read", async () => {
    await render(
      <a href='/apps/stock'>
        <AppMark glyph={Box} className='extra' />
        <span>Stock and replenishment</span>
      </a>
    );
    const mark = $("[data-app-mark]") as HTMLElement;
    expect(mark.tagName).toBe("SPAN");
    expect(mark.getAttribute("aria-hidden")).toBe("true");
    expect(mark.classList).toContain("extra");
    expect(mark.textContent).toBe("");
    // The link's name is the app's name alone: nothing in the mark is announced.
    const named = [...($("a") as HTMLElement).querySelectorAll("*")]
      .filter((el) => !el.closest("[aria-hidden='true']"))
      .map((el) => el.textContent)
      .join("");
    expect(named).toBe("Stock and replenishment");
  });
});
