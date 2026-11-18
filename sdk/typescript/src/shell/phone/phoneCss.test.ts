// The phone kit's half of shell.css, checked as text: it themes only through
// the shell's tokens, keeps the type and touch floor, and styles every class
// the components render (a renamed class would otherwise fail silently, as an
// unstyled button on a phone).

import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { SEGMENT_FADE_PX } from "./segmentedScroll";

const here = dirname(fileURLToPath(import.meta.url));
const raw = readFileSync(join(here, "..", "shell.css"), "utf8");
const START = "Phone kit (usePhoneLayout";
const section = (() => {
  const at = raw.indexOf(START);
  return at < 0 ? "" : raw.slice(at).replace(/\/\*[\s\S]*?\*\//g, "");
})();

/** The declarations of the first rule whose selector list includes `selector`. */
function rule(selector: string): string {
  for (const m of section.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const selectors = m[1].split(",").map((s) => s.trim());
    if (selectors.includes(selector)) return m[2];
  }
  throw new Error(`no rule for ${selector}`);
}

const sources = readdirSync(here)
  .filter((f) => /\.tsx?$/.test(f) && !/\.test\.tsx?$/.test(f) && !f.includes("harness"))
  .map((f) => readFileSync(join(here, f), "utf8"))
  .join("\n");

describe("shell.css phone kit", () => {
  it("is a section of shell.css", () => {
    expect(section.length).toBeGreaterThan(1000);
  });

  it("uses no colour literal, only the shell tokens", () => {
    const literals = section.match(/#[0-9a-fA-F]{3,8}\b|\b(?:rgba?|hsla?|oklch|oklab|lab|lch)\(/g);
    expect(literals).toBeNull();
    const light = raw.slice(
      raw.indexOf(".oxy-shell-scope {"),
      raw.indexOf(".dark .oxy-shell-scope")
    );
    const defined = new Set([...light.matchAll(/(--oxy-shell-[a-z-]+):/g)].map((m) => m[1]));
    const used = new Set([...section.matchAll(/var\((--oxy-shell-[a-z-]+)/g)].map((m) => m[1]));
    expect(used.size).toBeGreaterThan(5);
    for (const token of used) expect(defined, `${token} is not a shell token`).toContain(token);
  });

  it("sets no text under 11px", () => {
    const sizes = [...section.matchAll(/font-size:\s*(\d+(?:\.\d+)?)px/g)].map((m) => Number(m[1]));
    expect(sizes.length).toBeGreaterThan(10);
    for (const size of sizes) expect(size).toBeGreaterThanOrEqual(11);
  });

  it("raises fields to 16px inside the phone scope, and never lowers one", () => {
    // In the base layer an app's utilities and unlayered CSS outrank it, so a
    // large field keeps its size; small Tailwind sizes are raised by token.
    expect(section).toMatch(
      /@layer base \{\s*\.oxy-phone input,\s*\.oxy-phone select,\s*\.oxy-phone textarea \{\s*font-size: max\(1em, 16px\);/
    );
    expect(section).toMatch(/--text-sm: 16px;/);
  });

  it("lets a field set large keep its size in the shell-wide rule too", () => {
    const at = raw.indexOf("iOS Safari zooms");
    expect(at).toBeGreaterThan(0);
    const block = raw
      .slice(at, raw.indexOf("@keyframes oxy-today-in"))
      .replace(/\/\*[\s\S]*?\*\//g, "");
    // A class allowlist let text-4xl, text-[20px] and any CSS class shrink to 16px.
    expect(block).not.toMatch(/class\*=/);
    expect(block).toMatch(/@layer base \{[\s\S]*font-size: max\(1em, 16px\)/);
    expect(block).toMatch(/--text-xs: 16px;[\s\S]*--text-sm: 16px;/);
  });

  it("keeps 9999px only for the two dots, and takes the app's font", () => {
    const pills = [...section.matchAll(/([^{}]+)\{[^{}]*border-radius: 9999px/g)].map((m) =>
      m[1].trim()
    );
    expect(pills.sort()).toEqual([".oxy-phone-row__dot", ".oxy-phone-tabbar__dot"]);
    expect(section).toMatch(/font-family: var\(--font-sans, var\(--oxy-shell-font\)\)/);
  });

  it("styles every class the components render", () => {
    const named = new Set(
      [...sources.matchAll(/(?<![\w-])oxy-phone[a-z0-9_-]*/g)]
        .map((m) => m[0])
        .filter((c) => !c.endsWith("-"))
    );
    // Built from a prop: `oxy-phone-btn--${tone}`, `oxy-phone-tone--${tone}`.
    for (const tone of ["primary", "secondary", "danger"]) named.add(`oxy-phone-btn--${tone}`);
    for (const tone of ["good", "warn", "bad", "neutral"]) named.add(`oxy-phone-tone--${tone}`);
    expect(named.size).toBeGreaterThan(40);
    const styled = new Set([...section.matchAll(/\.(oxy-phone[a-z0-9_-]*)/g)].map((m) => m[1]));
    const missing = [...named].filter((c) => !styled.has(c));
    expect(missing).toEqual([]);
  });

  it("keeps the touch floor", () => {
    expect(rule(".oxy-phone-tabbar")).toMatch(/height: calc\(56px \+ env\(safe-area-inset-bottom/);
    expect(rule(".oxy-phone-tabbar__slot")).toMatch(/min-width: var\(--control-h-lg, 44px\)/);
    expect(rule(".oxy-phone-btn")).toMatch(/min-height: 48px/);
    expect(rule(".oxy-phone-row")).toMatch(/min-height: 56px/);
    expect(rule(".oxy-phone-segmented")).toMatch(/min-height: 48px/);
    expect(rule(".oxy-phone-sheet__back")).toMatch(/height: var\(--control-h-lg, 44px\)/);
    // A 24px ⓘ with a 10px reach on every side is a 44px target.
    expect(rule(".oxy-phone-hint")).toMatch(/height: 24px/);
    expect(rule(".oxy-phone-hint::before")).toMatch(/inset: -10px/);
  });

  it("makes every control at least 44px to tap, drawn or reached", () => {
    // Measured on phones in 2.27.1: a chip 36 tall, a segment 40, a row's button
    // 39 inside its 56px row, a one-line card's button 20.
    expect(rule(".oxy-phone-chip")).toMatch(/\bheight: var\(--control-h-lg, 44px\)/);
    expect(section).not.toMatch(/\.oxy-phone-chip::before/);
    expect(rule(".oxy-phone-segmented__option")).toMatch(/min-height: var\(--control-h-lg, 44px\)/);
    expect(rule(".oxy-phone-segmented")).toMatch(/padding: 2px/);

    // The row's vertical 8px and left 14px move onto its button, which stretches
    // to the row's height; the trailing slot and chevron keep the 8px as margin.
    expect(rule(".oxy-phone-row")).toMatch(/padding: 8px 14px;/);
    expect(rule(".oxy-phone-row--button")).toMatch(/padding: 0 14px 0 0;/);
    const main = rule(".oxy-phone-row--button .oxy-phone-row__main");
    expect(main).toMatch(/align-self: stretch;/);
    expect(main).toMatch(/padding: 8px 0 8px 14px;/);
    expect(main).toMatch(/box-sizing: border-box;/);
    for (const side of [".oxy-phone-row__trailing", ".oxy-phone-row__chevron"]) {
      const r = rule(`.oxy-phone-row--button ${side}`);
      expect(r).toMatch(/margin-top: 8px;/);
      expect(r).toMatch(/margin-bottom: 8px;/);
    }

    // The card's button takes the card's 12px above and below and hands it back.
    expect(rule(".oxy-phone-card")).toMatch(/padding: 12px 14px;/);
    const card = rule(".oxy-phone-card--button .oxy-phone-card__main");
    for (const d of [
      "margin-top: -12px",
      "margin-bottom: -12px",
      "padding-top: 12px",
      "padding-bottom: 12px"
    ])
      expect(card).toContain(d);

    // The ⓘ's reach sits above the row after it.
    expect(rule(".oxy-phone-hint::before")).toMatch(/z-index: 1;/);
  });

  it("pins the dock to the bottom of the scroller and clears the home indicator", () => {
    expect(rule(".oxy-phone-page__dock")).toMatch(/position: sticky;\s*bottom: 0/);
    expect(rule(".oxy-phone-page")).not.toMatch(/overflow/);
    expect(rule(".oxy-phone-tabbar")).toMatch(
      /env\(safe-area-inset-bottom, 0px\)\s*env\(safe-area-inset-left/
    );
  });

  it("keeps a row's whole height anywhere in a sheet, with the theme", () => {
    // The sheet's body is a scrolling flex column; a row under a `display: contents` wrapper is
    // still its item, and the row's 56px minimum would otherwise let it squeeze to 56px. Measured
    // in Chrome for the PR; jsdom lays nothing out, so the rule is held as text.
    expect(rule(".oxy-phone-sheet__body > *")).toMatch(/flex-shrink: 0;/);
    expect(rule(".oxy-phone-sheet__body .oxy-phone-row")).toMatch(
      /flex-shrink: calc\(1 - var\(--oxy-shell-themed, 0\)\);/
    );
  });

  it("fades the sheet in without travel under reduced motion", () => {
    const at = section.indexOf("@media (prefers-reduced-motion: reduce)");
    expect(at).toBeGreaterThanOrEqual(0);
    expect(section.slice(at)).toMatch(/\.oxy-phone-sheet \{\s*animation-name: oxy-phone-fade-in;/);
  });

  // Round 9 (the polish audit's X2 and X3). These hold with or without the theme: each acts only
  // where 2.28 cut a label or a value, so a row or a value that fits lays out as it did (the
  // no-theme pixel proof in the PR). jsdom lays nothing out, so the rules are held as text.
  it("sizes segments by their labels where an equal share would cut one, and scrolls rather than cut", () => {
    const row = rule(".oxy-phone-segmented");
    // A 1fr track never drops below its max-content minimum: equal while every label fits.
    expect(row).toMatch(/grid-auto-columns: minmax\(max-content, 1fr\);/);
    // As shrinkable as 2.28's row in a flex or grid parent, so a long row overflows itself.
    expect(row).toMatch(/min-width: 0;/);
    // A row that fits is no scroll container: one repaints its edges and labels a shade apart
    // (measured in Chrome, 1/255 on 24 pixels of a fitting row).
    expect(row).not.toMatch(/overflow|scrollbar/);
    // Only a row PhoneSegmented marks as wider than its box scrolls, without a bar.
    const fade = rule(".oxy-phone-segmented[data-oxy-fade]");
    expect(fade).toMatch(/overflow-x: auto;/);
    expect(fade).toMatch(/scrollbar-width: none;/);
    expect(rule(".oxy-phone-segmented[data-oxy-fade]::-webkit-scrollbar")).toMatch(
      /display: none;/
    );
    // The fade, on the edge PhoneSegmented marks, over the width it keeps the selection clear of.
    expect(fade).toMatch(/-webkit-mask-image: var\(--oxy-phone-segmented-fade\);/);
    expect(fade).toMatch(/(^|\s)mask-image: var\(--oxy-phone-segmented-fade\);/);
    const px = `${SEGMENT_FADE_PX}px`;
    expect(rule('.oxy-phone-segmented[data-oxy-fade~="start"]')).toContain(
      `--oxy-phone-segmented-fade-start: ${px};`
    );
    expect(rule('.oxy-phone-segmented[data-oxy-fade~="end"]')).toContain(
      `--oxy-phone-segmented-fade-end: ${px};`
    );
    // The reach is the option's own: 44px tall, as before.
    expect(rule(".oxy-phone-segmented__option")).toMatch(/min-height: var\(--control-h-lg, 44px\)/);
  });

  it("never cuts a card figure's value through a glyph", () => {
    const dd = rule(".oxy-phone-card__figure dd");
    // Its parts wrap rather than run past the column's edge; one line stays one line.
    expect(dd).toMatch(/flex-wrap: wrap;/);
    expect(dd).toMatch(/white-space: nowrap;/);
    expect(dd).toMatch(/overflow: hidden;/);
    // Text alone is a block, so it ends in an ellipsis (an anonymous flex item cannot).
    const text = rule(".oxy-phone-card__figure dd:not(:has(> *))");
    expect(text).toMatch(/display: block;/);
    expect(text).toMatch(/text-overflow: ellipsis;/);
    // An element of text alone wraps inside itself, and is never clipped at its own box: that
    // cuts the ink of a glyph past its advance in a value that fits (a bold "—", an "f"; measured
    // in Chrome for the PR, 10-31 pixels at up to 168/255 with overflow-x: clip).
    const leaf = rule(".oxy-phone-card__figure dd > :where(:not(:empty):not(:has(*)))");
    expect(leaf).toMatch(/white-space: normal;/);
    expect(leaf).toMatch(/overflow-wrap: anywhere;/);
    expect(leaf).not.toMatch(/overflow(-[xy])?:|text-overflow/);
  });
});
