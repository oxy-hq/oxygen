// @vitest-environment node
//
// PhoneOptionSheet's contrast cases, with the theme, in every direction theme.css ships, light
// and dark. Its rows sit on the kit's card surface, the foreground mixed into the page, which is
// no theme token, so the share is read out of shell.css (and must stay PhoneCard's), and every
// state the rows draw is measured on it: a label, a detail, the chosen row's check, a row
// pressed, and the focus ring.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { composite, type Rgb, wcagRatio } from "../../theme/contrast-math";
import { themeColour, UI_THEMES, type UiMode } from "../../ui/contrast.harness";

const here = dirname(fileURLToPath(import.meta.url));
const css = readFileSync(join(here, "..", "shell.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
const rule = (selector: string) =>
  new RegExp(`(?:^|\\n)${selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}\\s*\\{([^}]*)\\}`).exec(
    css
  )?.[1] ?? "";
const MIX =
  /color-mix\(in srgb, var\(--oxy-shell-foreground\) (\d+)%, (?:var\(--oxy-shell-background\)|transparent)\)/;
const shareOf = (selector: string, prop = "background") => {
  const m = new RegExp(`${prop}: ${MIX.source};`).exec(rule(selector));
  return m ? Number(m[1]) / 100 : undefined;
};

const opaque = (id: string, mode: UiMode, name: string): Rgb => {
  const c = themeColour(id, mode, name);
  if (!c) throw new Error(`${name} is missing`);
  const page = themeColour(id, mode, "--background") ?? [1, 1, 1, 1];
  const ground: Rgb = page[3] >= 1 ? [page[0], page[1], page[2]] : composite(page, [1, 1, 1]);
  return c[3] >= 1 ? [c[0], c[1], c[2]] : composite(c, ground);
};
const mix = (top: Rgb, share: number, under: Rgb): Rgb => composite([...top, share], under);

describe("PhoneOptionSheet's surface", () => {
  it("is the kit's card surface, the share PhoneCard draws", () => {
    expect(shareOf(".oxy-phone-options")).toBeDefined();
    expect(shareOf(".oxy-phone-options")).toBe(shareOf(".oxy-phone-card"));
    expect(shareOf(".oxy-phone-options__option:active")).toBe(0.06);
  });
});

for (const id of UI_THEMES) {
  for (const mode of ["light", "dark"] as const) {
    describe(`${id} ${mode}: PhoneOptionSheet on the kit surface`, () => {
      const fg = opaque(id, mode, "--foreground");
      const bg = opaque(id, mode, "--background");
      const surface = mix(fg, shareOf(".oxy-phone-options") ?? 0, bg);
      const pressed = mix(fg, shareOf(".oxy-phone-options__option:active") ?? 0, surface);
      const cases: [string, Rgb, Rgb, number][] = [
        ["a row's label", fg, surface, 4.5],
        ["a row's detail", opaque(id, mode, "--muted-foreground"), surface, 4.5],
        ["the chosen row's check", fg, surface, 3],
        ["a row's label, pressed", fg, pressed, 4.5],
        ["a row's detail, pressed", opaque(id, mode, "--muted-foreground"), pressed, 4.5],
        ["the focus ring inside a row", opaque(id, mode, "--focus-ring-color"), surface, 3]
      ];
      for (const [what, ink, ground, min] of cases) {
        const ratio = wcagRatio(ink, ground);
        it(`${what} >= ${min} (WCAG ${ratio.toFixed(2)})`, () => {
          expect(ratio + 1e-9).toBeGreaterThanOrEqual(min);
        });
      }
    });
  }
}
