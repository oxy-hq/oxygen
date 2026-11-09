// The top bar's touch targets, checked as text: a 44px reach on each control,
// only under the phone layout's own query, so a landscape touch tablet (Store
// Ops' kiosk: coarse pointer, wider than 640 and taller than 500) and a desktop
// keep the bar exactly as it was.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { PHONE_LAYOUT_QUERY } from "./usePhoneLayout";

const here = dirname(fileURLToPath(import.meta.url));
const css = readFileSync(join(here, "..", "shell.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");

/** Every block opened by `@media <query> {`, as [start, end) offsets, braces matched. */
function mediaBlocks(query: string): [number, number][] {
  const head = `@media ${query} {`;
  const out: [number, number][] = [];
  for (let at = css.indexOf(head); at >= 0; at = css.indexOf(head, at + 1)) {
    let depth = 0;
    for (let i = at + head.length - 1; i < css.length; i++) {
      if (css[i] === "{") depth++;
      else if (css[i] === "}" && --depth === 0) {
        out.push([at, i + 1]);
        break;
      }
    }
  }
  return out;
}

const CONTROLS = [
  ".oxy-navsheet-trigger",
  ".oxy-clock--button",
  ".oxy-askbtn",
  ".oxy-updatepill",
  ".oxy-navsheet__close",
  ".oxy-topbar__right > a",
  ".oxy-topbar__right > button"
];

describe("top bar touch targets", () => {
  const blocks = mediaBlocks(PHONE_LAYOUT_QUERY);
  const inPhone = (at: number) => blocks.some(([a, b]) => at > a && at < b);

  it("live in a block keyed on the phone layout's own query", () => {
    expect(PHONE_LAYOUT_QUERY).toBe(
      "(max-width: 639.98px), (pointer: coarse) and (max-height: 500px)"
    );
    const reach = blocks.filter(([a, b]) =>
      css.slice(a, b).includes(".oxy-navsheet-trigger::before")
    );
    expect(reach).toHaveLength(1);
  });

  it("give every bar control a reach at least 44px each way, centred on it", () => {
    for (const sel of CONTROLS) {
      const at = css.indexOf(`${sel}::before`);
      expect(at, `${sel}::before`).toBeGreaterThan(0);
      const body = css.slice(css.indexOf("{", at), css.indexOf("}", at));
      expect(body).toMatch(/content: "";/);
      expect(body).toMatch(/width: max\(100%, 44px\);/);
      expect(body).toMatch(/height: max\(100%, 44px\);/);
      expect(body).toMatch(/transform: translate\(-50%, -50%\);/);
    }
  });

  it("touch nothing outside the phone layout", () => {
    for (const sel of CONTROLS) {
      for (const suffix of ["::before", " {", ",\n"]) {
        const needle = `${sel}${suffix}`;
        for (let at = css.indexOf(needle); at >= 0; at = css.indexOf(needle, at + 1)) {
          const rule = css.slice(at, css.indexOf("}", at));
          // The control's own drawn rules (size, colour) sit outside on purpose;
          // only a reach or the position it needs must be phone-only.
          if (/::before|position: relative/.test(rule))
            expect(inPhone(at), `${needle} @${at}`).toBe(true);
        }
      }
    }
  });

  it("space the upright Today icon so a 32px neighbour's reach does not take from it", () => {
    const at = css.indexOf(".oxy-today-anchor {\n      margin-left: 2px;");
    expect(at).toBeGreaterThan(0);
    expect(inPhone(at)).toBe(true);
    const inner = css.lastIndexOf("@media (max-width: 639.98px) {", at);
    expect(inner).toBeGreaterThan(0);
    expect(inPhone(inner)).toBe(true);
    // 14px (half of 28) + 2 + 12 (the bar's gap) + 16 (half of 32) = 44.
    expect(css.slice(at, css.indexOf("}", at))).toMatch(/margin-right: 2px;/);
  });

  it("let an app keep its own control's position (base layer)", () => {
    const at = css.indexOf(".oxy-topbar__right > button::before");
    const before = css.slice(0, at);
    expect(before.lastIndexOf("@layer base {")).toBeGreaterThan(
      before.lastIndexOf(`@media ${PHONE_LAYOUT_QUERY} {`)
    );
  });
});
