// Shared by the desk UI's jsdom tests (not itself a test file): ui.css painted onto the jsdom
// document, so a test reads what an element computes to where two rules meet. jsdom's cascade
// takes specificity and source order, and ui.css is one layer (`oxy-ui`) in build order
// (scripts/build-ui-css.mjs), so within it order and specificity decide, as they do here. jsdom
// resolves neither a var() inside a value nor a media query, so the sheet is written out with
// every theme name replaced by its value in one mode (theme.css, the shipped direction), each
// calc() of lengths worked out, and the media blocks a test names opened as plain rules (every
// other media block is dropped, as jsdom would ignore it anyway).

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { uiCssSources } from "../../scripts/build-ui-css.mjs";
import { ALIAS_SELECTOR, cssBlockMap, themeSelectors } from "../theme/css-blocks";

export type PaintMode = "light" | "dark";

const here = dirname(fileURLToPath(import.meta.url));
const theme = cssBlockMap(readFileSync(join(here, "..", "theme", "theme.css"), "utf8"));
const selectors = themeSelectors("reference", "reference");
const light = theme.get(selectors.light) ?? new Map<string, string>();
const dark = theme.get(selectors.dark) ?? new Map<string, string>();
const aliases = theme.get(ALIAS_SELECTOR) ?? new Map<string, string>();

/** A theme name's value in one mode, every var() inside it resolved. */
export function themeValue(name: string, mode: PaintMode = "light"): string {
  const v = aliases.get(name) ?? (mode === "dark" ? dark.get(name) : undefined) ?? light.get(name);
  if (v === undefined) throw new Error(`${name} is not in theme.css`);
  return resolveValue(v, mode);
}

/** `value` with every var() replaced by the theme's value in `mode`, and calc() of lengths done. */
export function resolveValue(value: string, mode: PaintMode = "light", depth = 0): string {
  if (depth > 12) throw new Error(`var() nests too deep in ${value}`);
  const swapped = value.replace(/var\(\s*(--[a-z0-9-]+)\s*\)/g, (whole, name: string) => {
    const v =
      aliases.get(name) ?? (mode === "dark" ? dark.get(name) : undefined) ?? light.get(name);
    // A piece's own `--oxy-ui-*` property is left as written (jsdom keeps it unresolved).
    return v === undefined ? whole : resolveValue(v, mode, depth + 1);
  });
  return solveCalc(swapped).replace(/\s+/g, " ").trim();
}

/** Innermost first, each calc() whose terms are px, rem or plain numbers, as px. */
function solveCalc(value: string): string {
  let out = value;
  for (let guard = 0; guard < 20; guard += 1) {
    const next = out.replace(/calc\(([^()]*(?:\([^()]*\)[^()]*)*)\)/g, (whole, inner: string) => {
      const px = inner.replace(/(-?\d*\.?\d+)rem\b/g, (_, n: string) => String(Number(n) * 16));
      const bare = px.replace(/(-?\d*\.?\d+)px\b/g, "$1");
      if (!/^[\d.\s+\-*/()]+$/.test(bare)) return whole;
      // Only digits, operators and brackets remain (checked just above): safe to evaluate.
      // oxlint-disable-next-line typescript/no-implied-eval -- test-only harness; the input is checked to be arithmetic on numbers
      const n = Function(`"use strict"; return (${bare});`)() as number;
      return Number.isFinite(n) ? `${Math.round(n * 1000) / 1000}px` : whole;
    });
    if (next === out) return out;
    out = next;
  }
  return out;
}

/** Every top-level `@media <query> { … }` opened (its rules kept in place) when `open` names the
 *  query, dropped otherwise. */
function media(css: string, open: readonly string[]): string {
  const wanted = new Set(open.map((q) => q.replace(/\s+/g, " ").trim()));
  let out = "";
  let i = 0;
  for (;;) {
    const at = css.indexOf("@media", i);
    if (at < 0) return out + css.slice(i);
    const brace = css.indexOf("{", at);
    let depth = 1;
    let j = brace + 1;
    for (; j < css.length && depth > 0; j += 1) {
      if (css[j] === "{") depth += 1;
      else if (css[j] === "}") depth -= 1;
    }
    const query = css
      .slice(at + "@media".length, brace)
      .replace(/\s+/g, " ")
      .trim();
    out += css.slice(i, at) + (wanted.has(query) ? css.slice(brace + 1, j - 1) : "");
    i = j;
  }
}

/** ui.css's rules, base.css then each group in build order, comments removed. */
export function uiCssText(): string {
  return uiCssSources()
    .map((rel: string) => readFileSync(join(here, rel), "utf8"))
    .join("\n")
    .replace(/\/\*[\s\S]*?\*\//g, "");
}

export interface PaintOptions {
  mode?: PaintMode;
  /** Media queries to apply, as written in the CSS: `(min-width: 768px)`. */
  media?: readonly string[];
}

/** Paint ui.css onto `document` (replacing an earlier paint). */
export function paintUi({ mode = "light", media: open = [] }: PaintOptions = {}): void {
  unpaintUi();
  const style = document.createElement("style");
  style.dataset.oxyPaint = mode;
  style.textContent = media(uiCssText(), open).replace(
    /([a-z-]+)\s*:\s*([^;{}]*var\([^;{}]*)/g,
    (_, prop: string, value: string) => `${prop}: ${resolveValue(value, mode)}`
  );
  document.head.append(style);
}

export function unpaintUi(): void {
  for (const el of document.head.querySelectorAll("style[data-oxy-paint]")) el.remove();
}

/** A theme colour as jsdom prints a computed one: `#865108` as `rgb(134, 81, 8)`, an `rgba()`
 *  with its commas spaced. */
export function rgb(colour: string): string {
  const hex = /^#([0-9a-f]{6})$/i.exec(colour.trim())?.[1];
  if (!hex) return colour.trim().replace(/\s*,\s*/g, ", ");
  const n = (at: number) => Number.parseInt(hex.slice(at, at + 2), 16);
  return `rgb(${n(0)}, ${n(2)}, ${n(4)})`;
}
