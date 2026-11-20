// @vitest-environment node
//
// The one forms state the contrast harness cannot name, because forms.css draws it as a mix: a
// pressed ToggleChip on hover, its `--link` label over `--link` at 10% (`color-mix(in srgb,
// var(--link) 10%, transparent)`) over the ground it sits on. Measured with the harness's colours
// and the theme's maths, every direction, both modes; the label is 10px caps text (4.5:1).

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { composite, type Rgb, wcagRatio } from "../../theme/contrast-math";
import { themeColour, UI_THEMES, type UiMode } from "../contrast.harness";

const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "forms.css"), "utf8");

function ground(id: string, mode: UiMode, layers: string[]): Rgb {
  let rgb: Rgb = [1, 1, 1];
  for (const name of [...layers, "--background"].reverse()) {
    const c = themeColour(id, mode, name);
    if (!c) throw new Error(`${name} is missing in ${id} ${mode}`);
    rgb = composite(c, rgb);
  }
  return rgb;
}

/** The wash forms.css draws behind a hovered pressed chip: `--link` at this share. */
const HOVER =
  /\.oxy-ui-toggle-chip\[aria-pressed="true"\]:hover:not\(:disabled\) \{\s*background-color: color-mix\(in srgb, var\(--link\) (\d+)%, transparent\);/.exec(
    css
  );
const share = Number(HOVER?.[1] ?? Number.NaN) / 100;

describe("ToggleChip on, hover", () => {
  it("is drawn as a share of --link behind the --link label", () => {
    expect(HOVER).not.toBeNull();
    expect(share).toBeGreaterThan(0);
  });

  for (const id of UI_THEMES)
    for (const mode of ["light", "dark"] as const)
      for (const [where, layers] of [
        ["on a card", ["--card"]],
        ["on the page", []]
      ] as const)
        it(`${id} ${mode}: the label on its hover wash, ${where}, >= 4.5`, () => {
          const under = ground(id, mode, [...layers]);
          const link = themeColour(id, mode, "--link");
          if (!link) throw new Error("--link is missing");
          const ink = composite(link, under);
          const wash = composite([link[0], link[1], link[2], link[3] * share], under);
          expect(wcagRatio(ink, wash)).toBeGreaterThanOrEqual(4.5);
        });
});
