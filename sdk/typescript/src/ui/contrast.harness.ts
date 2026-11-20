// The desk UI's contrast harness: a case names an ink (or a ring, or an edge), the piece's own
// fill under it, and the grounds the piece sits on, by theme token; this measures it on the
// colours theme.css actually ships, every direction, both modes, the way the theme's own
// obligations are measured (`src/theme/contrast.test.ts`, the maths in `contrast-math.ts`).
//
// Cases live beside the pieces they draw: `src/ui/contrast.cases.ts` for the recipes, and one
// `contrast.cases.ts` per group folder. `contrast.test.ts` runs them all. Not exported from any
// entry; test support only.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { composite, parseColor, type Rgb, type Rgba, wcagRatio } from "../theme/contrast-math";
import { ALIAS_SELECTOR, cssBlockMap, themeSelectors } from "../theme/css-blocks";
import { THEMES } from "../theme/themes.gen";

/** What the reading is held to: WCAG 1.4.3 text (4.5), large text (3), 1.4.11 non-text, a focus
 *  indicator and a control's edge (3); `inactive` is a disabled control, which 1.4.3 exempts, held
 *  to a floor that keeps it visible as a word (2). */
export type UiContrastKind =
  | "text"
  | "large-text"
  | "non-text"
  | "focus"
  | "control-border"
  | "inactive";

export type UiMode = "light" | "dark";

export interface UiContrastCase {
  /** The piece, the part and the state: "Button primary: label on its fill, hover". */
  name: string;
  /** The theme token read: the ink, ring or edge. */
  fg: string;
  /** The piece's own surface under `fg` (a button's fill). Omit when `fg` sits on the ground. */
  fill?: string;
  /** The grounds under the piece, topmost first (`["--row-hover", "--card"]`). Translucent layers
   *  composite down the stack; the last one sits on `--background`, which sits on white. */
  on: string[];
  /** The piece's opacity (its fill and its ink together), e.g. 0.5 for a disabled button. */
  opacity?: number;
  /** The ratio it must reach. */
  min: number;
  kind: UiContrastKind;
  /** Only these modes (default both). */
  modes?: UiMode[];
}

export interface UiReading {
  ratio: number;
}

const here = dirname(fileURLToPath(import.meta.url));
const css = cssBlockMap(readFileSync(join(here, "..", "theme", "theme.css"), "utf8"));
const aliases = css.get(ALIAS_SELECTOR) ?? new Map<string, string>();
const shipped = THEMES[0].id;

/** One direction's colour for a token in one mode, through the derived aliases. */
export function themeColour(id: string, mode: UiMode, name: string, depth = 0): Rgba | undefined {
  const alias = /^var\((--[a-z0-9-]+)\)$/.exec(aliases.get(name) ?? "");
  if (alias && depth < 10) return themeColour(id, mode, alias[1], depth + 1);
  const sel = themeSelectors(id, shipped);
  const light = css.get(sel.light);
  const dark = mode === "dark" ? css.get(sel.dark) : undefined;
  return parseColor(dark?.get(name) ?? light?.get(name) ?? "");
}

const WHITE: Rgb = [1, 1, 1];
const over = (top: Rgba, under: Rgb): Rgb =>
  top[3] >= 1 ? [top[0], top[1], top[2]] : composite(top, under);

/** The opaque ground a stack of layers makes, topmost first, over --background over white. */
function ground(id: string, mode: UiMode, layers: string[]): Rgb | undefined {
  const page = themeColour(id, mode, "--background");
  if (!page) return undefined;
  let rgb = over(page, WHITE);
  for (const name of [...layers].reverse()) {
    const c = themeColour(id, mode, name);
    if (!c) return undefined;
    rgb = over(c, rgb);
  }
  return rgb;
}

/** `top` drawn at `alpha` over an opaque `under`. */
const at = (top: Rgb, alpha: number, under: Rgb): Rgb => composite([...top, alpha], under);

/** The case's ratio in one direction and mode, or undefined when a token is missing. */
export function measureUi(id: string, mode: UiMode, c: UiContrastCase): UiReading | undefined {
  const base = ground(id, mode, c.on);
  const fg = themeColour(id, mode, c.fg);
  const fill = c.fill ? themeColour(id, mode, c.fill) : ([0, 0, 0, 0] as Rgba);
  if (!base || !fg || !fill) return undefined;
  const opacity = c.opacity ?? 1;
  // The piece is drawn, then laid on its ground at its opacity (source-over, gamma-encoded).
  const fillA = fill[3];
  const fgA = fg[3];
  const outA = fgA + fillA * (1 - fgA);
  const fgOverFill: Rgb =
    outA === 0
      ? [0, 0, 0]
      : ([0, 1, 2].map((i) => (fg[i] * fgA + fill[i] * fillA * (1 - fgA)) / outA) as Rgb);
  const behind = fillA > 0 ? at([fill[0], fill[1], fill[2]], fillA * opacity, base) : base;
  const ink = at(fgOverFill, outA * opacity, base);
  return { ratio: wcagRatio(ink, behind) };
}

/** Every direction theme.css ships, by id. */
export const UI_THEMES: readonly string[] = THEMES.map((t) => t.id);

export const caseLabel = (id: string, mode: UiMode, c: UiContrastCase) =>
  `${id} ${mode} ${c.name}: ${c.fg}${c.fill ? ` on ${c.fill}` : ""} over ${c.on.join(" over ")}${c.opacity !== undefined ? ` at ${c.opacity}` : ""} (${c.kind})`;
