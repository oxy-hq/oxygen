// The touch rules in index.css, checked as text: the 16px field rule can only
// raise a size, and a switch's extra tap area stays small enough not to reach
// the control beside it. Measured in a browser when written; this keeps them so.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const here = dirname(fileURLToPath(import.meta.url));
const css = readFileSync(join(here, "index.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");

function block(start: string): string {
  const at = css.indexOf(start);
  expect(at, `no ${start}`).toBeGreaterThanOrEqual(0);
  let depth = 0;
  for (let i = css.indexOf("{", at); i < css.length; i++) {
    if (css[i] === "{") depth++;
    if (css[i] === "}" && --depth === 0) return css.slice(at, i + 1);
  }
  throw new Error("unbalanced");
}

describe("touch rules", () => {
  it("raises small fields through Tailwind's tokens and the base layer, and never lowers one", () => {
    const rule = block("@media (pointer: coarse) and (max-width: 767px)");
    // A class allowlist let text-4xl and text-[20px] shrink to 16px.
    expect(rule).not.toMatch(/class\*=/);
    expect(rule).toMatch(/--text-xs: 16px;[\s\S]*--text-sm: 16px;/);
    expect(rule).toMatch(/@layer base \{[\s\S]*font-size: max\(1em, 16px\)/);
  });

  it("keeps a switch's tap margin inside the 24px minimum", () => {
    const after = block('[data-slot="switch"]::after');
    const inset = after.match(/inset: -(\d+)px(?: -(\d+)px)?/);
    expect(inset).not.toBeNull();
    expect(Number(inset?.[1])).toBeLessThanOrEqual(3);
    expect(Number(inset?.[2] ?? inset?.[1])).toBeLessThanOrEqual(3);
  });
});
