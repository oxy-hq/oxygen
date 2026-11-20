// @vitest-environment node
//
// The one figures state the contrast harness cannot name, because figures.css draws it as a mix:
// a Chip's remove × on hover, at full ink over its own ink at 15% (`color-mix(in srgb,
// currentColor 15%, transparent)`) over the chip's tint over a card. Measured with the harness's
// colours and the theme's maths, every direction, both modes; the × is a graphic (1.4.11, 3:1).

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { composite, type Rgb, wcagRatio } from "../../theme/contrast-math";
import { themeColour, UI_THEMES, type UiMode } from "../contrast.harness";

const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "figures.css"), "utf8");

/** An opaque ground: layers topmost first, over --background, over white. */
function ground(id: string, mode: UiMode, layers: string[]): Rgb {
  let rgb: Rgb = [1, 1, 1];
  for (const name of [...layers, "--background"].reverse()) {
    const c = themeColour(id, mode, name);
    if (!c) throw new Error(`${name} is missing in ${id} ${mode}`);
    rgb = composite(c, rgb);
  }
  return rgb;
}

const INK = {
  success: ["--status-success", "--status-success-bg"],
  warning: ["--status-warning", "--status-warning-bg"],
  error: ["--status-error", "--status-error-bg"],
  info: ["--status-info", "--status-info-bg"],
  neutral: ["--dim-foreground", "--status-neutral-bg"]
} as const;

/** The wash figures.css draws behind a hovered ×: its own ink at this share. */
const HOVER =
  /\.oxy-ui-chip__remove:hover:not\(:disabled\) \{\s*background-color: color-mix\(in srgb, currentColor (\d+)%, transparent\);\s*opacity: 1;/.exec(
    css
  );
const share = Number(HOVER?.[1] ?? Number.NaN) / 100;

describe("Chip remove ×, hover", () => {
  it("is drawn as the chip's own ink, a share of it behind a full-ink ×", () => {
    expect(HOVER).not.toBeNull();
    expect(share).toBeGreaterThan(0);
  });

  for (const id of UI_THEMES)
    for (const mode of ["light", "dark"] as const)
      for (const [tone, [ink, tint]] of Object.entries(INK))
        it(`${id} ${mode} ${tone}: the × against its hover wash, on its tint on a card, >= 3`, () => {
          const under = ground(id, mode, [tint, "--card"]);
          const fg = themeColour(id, mode, ink);
          if (!fg) throw new Error(`${ink} is missing`);
          const inkRgb = composite(fg, under);
          const wash = composite([inkRgb[0], inkRgb[1], inkRgb[2], share], under);
          expect(wcagRatio(inkRgb, wash)).toBeGreaterThanOrEqual(3);
        });
});
