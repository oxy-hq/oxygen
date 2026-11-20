// @vitest-environment node
//
// The recipes: `ui`'s two spellings name the same classes, the tone helpers keep the reference's
// rules, and base.css draws each recipe at the reference's values. That last part is the
// translation's proof: each row is a reference recipe (oxy-hq/reference-app@3caad1f
// `src/components/ui.ts`) with the value its Tailwind class computes to, and the test resolves
// base.css's rule for that recipe through theme.css (light, and dark for colours) and compares.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { ALIAS_SELECTOR, cssBlockMap, themeSelectors } from "../theme/css-blocks";
import { ui } from "./recipes";
import {
  chipTone,
  phoneToneOf,
  STATUS_TONES,
  type StatusTone,
  statusToneOf,
  strongestTone
} from "./tone";

const here = dirname(fileURLToPath(import.meta.url));
const theme = cssBlockMap(readFileSync(join(here, "..", "theme", "theme.css"), "utf8"));
const selectors = themeSelectors("reference", "reference");
const light = theme.get(selectors.light) ?? new Map<string, string>();
const dark = theme.get(selectors.dark) ?? new Map<string, string>();
const aliases = theme.get(ALIAS_SELECTOR) ?? new Map<string, string>();
const base = readFileSync(join(here, "base.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");

/** The declarations of the rule whose selector is exactly `selector` (the last one, as the
 *  cascade would take it within the file). */
function decls(selector: string): Map<string, string> {
  const out = new Map<string, string>();
  for (const m of base.matchAll(/([^{};]+)\{([^{}]*)\}/g)) {
    if (m[1].replace(/\s+/g, " ").trim() !== selector) continue;
    for (const d of m[2].matchAll(/([a-z-]+)\s*:\s*([^;]+);/g))
      out.set(d[1], d[2].replace(/\s+/g, " ").trim());
  }
  return out;
}

/** A value with every var() replaced by the theme's value in one mode. */
function resolve(value: string, mode: "light" | "dark" = "light", depth = 0): string {
  return value.replace(/var\((--[a-z0-9-]+)\)/g, (_, name: string) => {
    const alias = aliases.get(name);
    if (alias && depth < 10) return resolve(alias, mode, depth + 1);
    const v = (mode === "dark" ? dark.get(name) : undefined) ?? light.get(name);
    if (v === undefined) throw new Error(`${name} is not in theme.css`);
    return resolve(v, mode, depth + 1);
  });
}

/** [selector, property, light value, dark value when it differs]. The reference's Tailwind class
 *  is named in the comment of each group. */
const REFERENCE: [string, string, string, string?][] = [
  // focusRing: ring-[1.5px] ring-muted-foreground (#5a5a60 / #aaaab3), offset 0.
  [
    ".oxy-ui-btn--primary:focus-visible",
    "box-shadow",
    "0 0 0 1px #f6f6f7, 0 0 0 calc(1.5px + 1px) #5a5a60",
    "0 0 0 1px #0a0a0a, 0 0 0 calc(1.5px + 1px) #aaaab3"
  ],
  // spacingXs..Lg: gap-1, gap-2, gap-4, gap-8. paddingSm px-3 py-2, paddingMd p-4.
  [".oxy-ui-gap-xs", "gap", "4px"],
  [".oxy-ui-gap-sm", "gap", "8px"],
  [".oxy-ui-gap-md", "gap", "16px"],
  [".oxy-ui-gap-lg", "gap", "32px"],
  [".oxy-ui-pad-sm", "padding", "8px 12px"],
  [".oxy-ui-pad-md", "padding", "16px"],
  [".oxy-ui-page", "padding", "16px"],
  [".oxy-ui-prose", "max-width", "48rem"],
  [".oxy-ui-form-page", "max-width", "36rem"],
  [".oxy-ui-form-page", "gap", "16px"],
  // card: border border-border-subtle bg-card.
  [
    ".oxy-ui-card",
    "border",
    "1px solid rgba(0, 0, 0, 0.08)",
    "1px solid rgba(255, 255, 255, 0.09)"
  ],
  [".oxy-ui-card", "background-color", "#ffffff", "#1a1a1a"],
  [".oxy-ui-card", "border-radius", "0px"],
  // panel: flex flex-col gap-2 p-4; panelHeader: gap-2 border-b border-border-subtle pb-2.
  [".oxy-ui-panel", "gap", "8px"],
  [".oxy-ui-panel", "padding", "16px"],
  [".oxy-ui-panel-header", "padding-bottom", "8px"],
  // tile: border border-border-subtle bg-field.
  [".oxy-ui-tile", "background-color", "#ffffff", "#252525"],
  // The type scale (index.css:1185-1210 with ui.ts:302-310).
  [".oxy-ui-text-2xl", "font-size", "2.25rem"],
  [".oxy-ui-text-2xl", "line-height", "1.25"],
  [".oxy-ui-text-2xl", "font-weight", "600"],
  [".oxy-ui-text-xl", "font-size", "1.5rem"],
  [".oxy-ui-text-xl", "line-height", "1.25"],
  [".oxy-ui-text-lg", "font-size", "1.25rem"],
  [".oxy-ui-text-lg", "font-weight", "500"],
  [".oxy-ui-text-base", "font-size", "1rem"],
  [".oxy-ui-text-base", "line-height", "1.375"],
  [".oxy-ui-text-md", "font-size", "0.875rem"],
  [".oxy-ui-text-md", "line-height", "1.4"],
  [".oxy-ui-text-sm", "font-size", "0.8125rem"],
  [".oxy-ui-text-xs", "font-size", "0.75rem"],
  [".oxy-ui-text-xs", "letter-spacing", "0.06em"],
  [".oxy-ui-text-2xs", "font-size", "0.6875rem"],
  [".oxy-ui-text-2xs", "letter-spacing", "0.04em"],
  [".oxy-ui-text-3xs", "font-size", "0.625rem"],
  // eyebrow: textXs text-subtle-foreground (#626269 / #95959d).
  [".oxy-ui-eyebrow", "font-size", "0.75rem"],
  [".oxy-ui-eyebrow", "line-height", "1.4"],
  [".oxy-ui-eyebrow", "font-weight", "600"],
  [".oxy-ui-eyebrow", "letter-spacing", "0.06em"],
  [".oxy-ui-eyebrow", "text-transform", "uppercase"],
  [".oxy-ui-eyebrow", "color", "#626269", "#95959d"],
  [".oxy-ui-note", "color", "#5a5a60", "#aaaab3"],
  [".oxy-ui-caption", "font-size", "0.6875rem"],
  [".oxy-ui-label", "font-size", "0.875rem"],
  [".oxy-ui-label", "color", "#444448", "#babac5"],
  [".oxy-ui-figure", "font-size", "1.25rem"],
  [".oxy-ui-figure", "font-weight", "500"],
  // Heights: min-h-8 / min-h-9 / min-h-11; controlSm px-3, controlLg px-4.
  [".oxy-ui-h-sm", "min-height", "32px"],
  [".oxy-ui-h-md", "min-height", "36px"],
  [".oxy-ui-h-lg", "min-height", "44px"],
  [".oxy-ui-control-sm", "padding-inline", "12px"],
  [".oxy-ui-control-lg", "padding-inline", "16px"],
  // btnCompact: min-h-8 px-3 gap-1.5 border text-md font-medium.
  [".oxy-ui-btn", "font-weight", "500"],
  [".oxy-ui-btn--sm", "min-height", "32px"],
  [".oxy-ui-btn--sm", "padding", "0 12px"],
  [".oxy-ui-btn--sm", "gap", "6px"],
  [".oxy-ui-btn--sm", "font-size", "0.875rem"],
  // btnPrimary / btnSecondary: min-h-11 px-4 gap-2 text-base.
  [".oxy-ui-btn--lg", "min-height", "44px"],
  [".oxy-ui-btn--lg", "padding", "0 16px"],
  [".oxy-ui-btn--lg", "gap", "8px"],
  [".oxy-ui-btn--lg", "font-size", "1rem"],
  // btnCompactPrimary: border-transparent bg-brand text-inverse-foreground hover:bg-brand/90.
  [".oxy-ui-btn--primary", "background-color", "#3550ff", "#445dff"],
  [".oxy-ui-btn--primary", "color", "#ffffff"],
  [
    '.oxy-ui-btn--primary:hover:not(:disabled, [aria-disabled="true"])',
    "background-color",
    "rgba(53, 80, 255, 0.9)",
    "rgba(68, 93, 255, 0.9)"
  ],
  // btnCompactSecondary: border-border-subtle bg-btn-surface text-foreground hover:bg-btn-surface-hover.
  [".oxy-ui-btn--secondary", "border-color", "rgba(0, 0, 0, 0.08)", "rgba(255, 255, 255, 0.09)"],
  [".oxy-ui-btn--secondary", "background-color", "#dcdce0", "#3a3a41"],
  [".oxy-ui-btn--secondary", "color", "#18181b", "#ededed"],
  [
    '.oxy-ui-btn--secondary:hover:not(:disabled, [aria-disabled="true"])',
    "background-color",
    "#d0d0d5",
    "#45454e"
  ],
  // btnText: text-dim-foreground hover:text-foreground, no border, no side padding.
  [".oxy-ui-btn--text", "color", "#444448", "#babac5"],
  [".oxy-ui-btn--text", "border-width", "0"],
  [".oxy-ui-btn--text", "padding-inline", "0"],
  ['.oxy-ui-btn--text:hover:not(:disabled, [aria-disabled="true"])', "color", "#18181b", "#ededed"],
  // disabled:opacity-50.
  [
    '.oxy-ui-btn:disabled, .oxy-ui-btn[aria-disabled="true"]:not([aria-busy="true"])',
    "opacity",
    "0.5"
  ],
  // input: min-h-11 px-4 py-2 border-field-border bg-field text-base; focus border, disabled.
  [".oxy-ui-field", "min-height", "44px"],
  [".oxy-ui-field", "padding", "8px 16px"],
  [
    ".oxy-ui-field",
    "border",
    "1px solid rgba(0, 0, 0, 0.42)",
    "1px solid rgba(255, 255, 255, 0.34)"
  ],
  [".oxy-ui-field", "background-color", "#ffffff", "#252525"],
  [".oxy-ui-field", "font-size", "1rem"],
  [".oxy-ui-field::placeholder", "color", "#5a5a60", "#aaaab3"],
  [".oxy-ui-field:focus", "border-color", "#5a5a60", "#aaaab3"],
  [".oxy-ui-field:disabled", "background-color", "#ececee", "#27272a"],
  [".oxy-ui-field:disabled", "color", "#94949d", "#626268"],
  // inputSm: min-h-8 px-3 text-md.
  [".oxy-ui-field--sm", "min-height", "32px"],
  [".oxy-ui-field--sm", "padding", "0 12px"],
  [".oxy-ui-field--sm", "font-size", "0.875rem"],
  // select: min-h-8 px-3 pr-8, chevron 12x8 at right 0.75rem.
  [".oxy-ui-select", "min-height", "32px"],
  [".oxy-ui-select", "padding", "0 32px 0 12px"],
  [".oxy-ui-select", "background-size", "12px 8px"],
  [".oxy-ui-select", "background-position", "right 0.75rem center"],
  [".oxy-ui-select--touch", "padding-right", "40px"],
  [".oxy-ui-select--touch", "background-position", "right 1rem center"],
  // errorInput: border-status-error.
  [
    '.oxy-ui-field--error, .oxy-ui-field--error:focus, :is(.oxy-ui-field, .oxy-ui-select)[aria-invalid="true"], :is(.oxy-ui-field, .oxy-ui-select)[aria-invalid="true"]:focus',
    "border-color",
    "#b01b1b",
    "#fa8e8e"
  ],
  // tag: px-1.5 py-px text-2xs leading-[1.65] font-semibold.
  [".oxy-ui-tag", "padding", "1px 6px"],
  [".oxy-ui-tag", "font-size", "0.6875rem"],
  [".oxy-ui-tag", "line-height", "1.65"],
  [".oxy-ui-tag", "font-weight", "600"],
  // chipTone.warning: bg-status-warning-bg text-status-warning.
  [".oxy-ui-tone-warning", "background-color", "rgba(234, 179, 8, 0.2)", "rgba(234, 179, 8, 0.14)"],
  [".oxy-ui-tone-warning", "color", "#865108", "#eab308"],
  [".oxy-ui-tone-neutral", "color", "#444448", "#babac5"],
  // callout: border px-[0.7rem] py-[0.3rem] text-xs leading-[1.45].
  [".oxy-ui-callout", "padding", "0.3rem 0.7rem"],
  [".oxy-ui-callout", "font-size", "0.75rem"],
  [".oxy-ui-callout", "line-height", "1.45"],
  [
    ".oxy-ui-callout--error",
    "border-color",
    "rgba(185, 28, 28, 0.32)",
    "rgba(250, 142, 142, 0.34)"
  ],
  // rowSelected bg-row-selected.
  [".oxy-ui-row-selected", "background-color", "rgba(0, 0, 0, 0.09)", "rgba(255, 255, 255, 0.09)"],
  // tab: min-h-9 gap-2 -mb-px border px-3; tabOn bg-background, border-b-background.
  [".oxy-ui-tab", "min-height", "36px"],
  [".oxy-ui-tab", "margin-bottom", "-1px"],
  [".oxy-ui-tab", "padding", "0 12px"],
  [".oxy-ui-tab--on", "background-color", "#f6f6f7", "#0a0a0a"],
  [".oxy-ui-tab--on", "border-bottom-color", "#f6f6f7", "#0a0a0a"],
  // topBarTab: border-b-2 px-2.5; on: border-foreground.
  [".oxy-ui-top-bar-tab", "border-width", "0 0 2px"],
  [".oxy-ui-top-bar-tab", "padding", "0 10px"],
  [".oxy-ui-top-bar-tab--on", "border-color", "#18181b", "#ededed"],
  // segmentRow / segment: min-h-9 px-3; on bg-segment-selected.
  [".oxy-ui-segment-row", "background-color", "#ffffff", "#252525"],
  [".oxy-ui-segment", "min-height", "36px"],
  [".oxy-ui-segment", "padding", "0 12px"],
  [".oxy-ui-segment--on", "background-color", "#e4e4e7", "#000000"],
  // tabCount: px-[5px] py-px text-xs font-semibold.
  [".oxy-ui-tab-count", "padding", "1px 5px"],
  [".oxy-ui-tab-count", "font-size", "0.75rem"],
  // cell: px-[0.6rem] py-[0.4rem] align-top text-dim-foreground.
  [".oxy-ui-cell", "padding", "0.4rem 0.6rem"],
  [".oxy-ui-cell", "vertical-align", "top"],
  [".oxy-ui-cell", "color", "#444448", "#babac5"],
  // sheetBand h-[1.55rem], sheetHeader h-10, sheetRow h-12.
  [".oxy-ui-sheet-band", "height", "1.55rem"],
  [".oxy-ui-sheet-band-offset", "top", "1.55rem"],
  [".oxy-ui-sheet-header", "height", "40px"],
  [".oxy-ui-sheet-row", "height", "48px"],
  // iconInline size-3.5, iconControl size-4.
  [".oxy-ui-icon-inline", "width", "14px"],
  [".oxy-ui-icon-control", "width", "16px"],
  // sectionLink: h-9 gap-2 px-2.5; on: bg-row-selected font-medium.
  [".oxy-ui-section-link", "height", "36px"],
  [".oxy-ui-section-link", "padding", "0 10px"],
  [".oxy-ui-section-link--on", "font-weight", "500"],
  // touchRow: min-h-11 px-4 py-3.
  [".oxy-ui-touch-row", "min-height", "44px"],
  [".oxy-ui-touch-row", "padding", "12px 16px"]
];

describe("base.css draws each recipe at the reference's values", () => {
  for (const [selector, prop, want, wantDark] of REFERENCE) {
    it(`${selector} { ${prop}: ${want}${wantDark ? ` / dark ${wantDark}` : ""} }`, () => {
      const value = decls(selector).get(prop);
      expect(value, `${selector} has no ${prop}`).toBeDefined();
      expect(resolve(value ?? "", "light")).toBe(want);
      expect(resolve(value ?? "", "dark")).toBe(wantDark ?? want);
    });
  }
});

describe("ui", () => {
  /** Every class string in `ui`, through its groups. */
  const strings = (v: unknown): string[] =>
    typeof v === "string"
      ? [v]
      : typeof v === "object" && v !== null
        ? Object.values(v).flatMap(strings)
        : [];

  it("names only .oxy-ui-* classes", () => {
    const all = strings(ui).flatMap((s) => s.split(" "));
    expect(all.length).toBeGreaterThan(100);
    for (const c of all) expect(c).toMatch(/^oxy-ui-[a-z0-9_-]+$/);
  });

  it("names the same classes in its grouped spellings as in the reference's flat names", () => {
    expect(ui.btn.primary).toBe(`${ui.btnCompact} ${ui.btnCompactPrimary}`);
    expect(ui.btn.secondary).toBe(`${ui.btnCompact} ${ui.btnCompactSecondary}`);
    expect(ui.btn.text).toBe(ui.btnText);
    expect(ui.btn.primaryLg).toBe(ui.btnPrimary);
    expect(ui.btn.secondaryLg).toBe(ui.btnSecondary);
    expect(ui.field).toBe(ui.input);
    expect(ui.fieldSm).toBe(ui.inputSm);
    expect(ui.fieldQuiet).toBe(ui.inputQuiet);
    expect(ui.fieldError).toBe(ui.errorInput);
    expect(ui.space).toEqual({
      xs: ui.spacingXs,
      sm: ui.spacingSm,
      md: ui.spacingMd,
      lg: ui.spacingLg,
      paddingSm: ui.paddingSm,
      paddingMd: ui.paddingMd
    });
    expect(ui.control).toEqual({
      heightSm: ui.heightSm,
      heightMd: ui.heightMd,
      heightLg: ui.heightLg,
      sm: ui.controlSm,
      lg: ui.controlLg
    });
    expect(ui.table).toEqual({
      cell: ui.cell,
      numCell: ui.numCell,
      band: ui.sheetBand,
      bandOffset: ui.sheetBandOffset,
      header: ui.sheetHeader,
      row: ui.sheetRow
    });
    expect(Object.values(ui.text)).toEqual([
      ui.text2xl,
      ui.textXl,
      ui.textLg,
      ui.textBase,
      ui.textMd,
      ui.textSm,
      ui.textXs,
      ui.text2xs,
      ui.text3xs
    ]);
    for (const tone of STATUS_TONES) expect(ui.chip[tone]).toBe(`${ui.tag} ${chipTone[tone]}`);
  });

  it("gives a row one state: selection outranks hover, never both", () => {
    expect(ui.rowState(true)).toBe(ui.rowSelected);
    expect(ui.rowState(false)).toBe(ui.rowHover);
  });
});

describe("tones", () => {
  it("wear the strongest status: error over warning over everything else, neutral as undefined", () => {
    expect(strongestTone(["success", "warning", "info"])).toBe("warning");
    expect(strongestTone(["warning", "error", "success"])).toBe("error");
    expect(strongestTone(["success", "info", "neutral", undefined, null])).toBeUndefined();
    expect(strongestTone([])).toBeUndefined();
  });

  it("map one-to-one onto the phone kit's words for the four it has", () => {
    for (const tone of ["success", "warning", "error", "neutral"] as StatusTone[])
      expect(statusToneOf[phoneToneOf[tone]]).toBe(tone);
    expect(phoneToneOf.info).toBe("neutral");
  });
});
