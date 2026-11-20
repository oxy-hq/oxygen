// @vitest-environment node
//
// The selected segment's case (contrast.cases.ts) is a check that can fail: measured on the
// colours BMG's Supplier Ordering shipped in dark mode (review round, 18 Nov: its report card's
// light palette on a plain `:root` kept `--segment-selected` at #e4e4e7 under the dark theme's
// white ink, 1.08:1), it fails; measured on the SDK's theme it passes. Same maths as the
// harness (WCAG 2.2, source-over composite down the stack onto --background).

import { describe, expect, it } from "vitest";
import { composite, parseColor, type Rgb, type Rgba, wcagRatio } from "../../theme/contrast-math";
import { themeColour, type UiContrastCase, type UiMode } from "../contrast.harness";
import { contrastCases } from "./contrast.cases";

const CASE = contrastCases.find((c) =>
  c.name.startsWith("Segmented selected: label on the selected fill, on the track, on a card")
);

/** BMG's leaked palette (reportCard.css, a plain `:root`, so it won in both modes). */
const BMG: Record<string, string> = { "--segment-selected": "#e4e4e7" };

function measure(c: UiContrastCase, mode: UiMode, overrides: Record<string, string> = {}) {
  const colour = (name: string): Rgba => {
    const v = overrides[name] ? parseColor(overrides[name]) : themeColour("reference", mode, name);
    if (!v) throw new Error(`no colour for ${name}`);
    return v;
  };
  const over = (top: Rgba, under: Rgb): Rgb =>
    top[3] >= 1 ? [top[0], top[1], top[2]] : composite(top, under);
  let ground = over(colour("--background"), [1, 1, 1]);
  for (const name of [...c.on].reverse()) ground = over(colour(name), ground);
  const behind = c.fill ? over(colour(c.fill), ground) : ground;
  return wcagRatio(over(colour(c.fg), behind), behind);
}

describe("the selected segment's contrast case", () => {
  it("exists, and reads the label on the selected fill", () => {
    expect(CASE?.fg).toBe("--foreground");
    expect(CASE?.fill).toBe("--segment-selected");
  });

  it("fails on BMG's dark-mode colours, as the review measured (1.08:1)", () => {
    const ratio = measure(CASE as UiContrastCase, "dark", BMG);
    expect(ratio).toBeLessThan(CASE?.min ?? 0);
    expect(ratio).toBeCloseTo(1.08, 2);
  });

  it("passes on the SDK's own theme, both modes", () => {
    for (const mode of ["light", "dark"] as const)
      expect(measure(CASE as UiContrastCase, mode)).toBeGreaterThanOrEqual(CASE?.min ?? Infinity);
  });
});
