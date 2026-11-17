// @vitest-environment node
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import pairsJson from "./contract/pairs.json";
import { ALIAS_SELECTOR, cssBlockMap, themeSelectors } from "./css-blocks";
import { THEMES } from "./themes.gen";

// Every contrast obligation in contract/pairs.json, for every direction in both modes, measured
// on the colours theme.css actually ships. WCAG 2.2 contrast ratio is the gate; APCA Lc is
// reported in each case's name and never asserted. Translucent colours are composited the way the
// design validator (validate_tokens.py) does it: source-over in gamma-encoded sRGB, the
// background over its stated `over` surface (default --background, which itself sits on white
// when translucent), then the foreground over that result.

interface Pair {
  fg: string;
  bg: string;
  over?: string;
  min: number;
  kind: string;
  modes?: string[];
}
type Rgb = [number, number, number];
type Rgba = [number, number, number, number];

/**
 * Obligations a current direction misses, as `direction mode fg on bg[ over surface] (kind)`.
 * Designers own the tokens, so a miss is recorded here, not fixed: each listed case runs as
 * `it.fails`, the suite stays green, and the case fails the moment the tokens are fixed, so the
 * entry must go. Empty: every direction meets every obligation in both modes.
 */
const KNOWN_FAILURES: readonly string[] = [];

const caseLabel = (id: string, mode: string, p: Pair) =>
  `${id} ${mode} ${p.fg} on ${p.bg}${p.over ? ` over ${p.over}` : ""} (${p.kind})`;

const here = dirname(fileURLToPath(import.meta.url));
const css = cssBlockMap(readFileSync(join(here, "theme.css"), "utf8"));
const pairs = pairsJson as Pair[];
const aliases = css.get(ALIAS_SELECTOR) ?? new Map<string, string>();
const shipped = THEMES[0].id;

const toLinear = (c: number) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
const luminance = ([r, g, b]: Rgb) =>
  0.2126 * toLinear(r) + 0.7152 * toLinear(g) + 0.0722 * toLinear(b);

function wcagRatio(a: Rgb, b: Rgb): number {
  const [la, lb] = [luminance(a), luminance(b)];
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

/** APCA-W3 0.0.98G-4g lightness contrast, signed (negative: light text on a dark ground). */
function apcaLc(text: Rgb, bg: Rgb): number {
  const y = ([r, g, b]: Rgb) => 0.2126729 * r ** 2.4 + 0.7151522 * g ** 2.4 + 0.072175 * b ** 2.4;
  // biome-ignore lint/suspicious/noApproximativeNumericConstant: APCA's black clamp is 1.414 by definition; Math.SQRT2 would change Lc
  const clampBlack = (v: number) => (v > 0.022 ? v : v + (0.022 - v) ** 1.414);
  const ty = clampBlack(y(text));
  const by = clampBlack(y(bg));
  if (Math.abs(by - ty) < 0.0005) return 0;
  if (by > ty) {
    const sapc = (by ** 0.56 - ty ** 0.57) * 1.14;
    return (sapc < 0.1 ? 0 : sapc - 0.027) * 100;
  }
  const sapc = (by ** 0.65 - ty ** 0.62) * 1.14;
  return (sapc > -0.1 ? 0 : sapc + 0.027) * 100;
}

const composite = ([r, g, b, a]: Rgba, [br, bg, bb]: Rgb): Rgb => [
  a * r + (1 - a) * br,
  a * g + (1 - a) * bg,
  a * b + (1 - a) * bb
];

function parseColor(value: string): Rgba | undefined {
  const hex = /^#([0-9a-f]{6})$/.exec(value);
  if (hex) {
    const n = Number.parseInt(hex[1], 16);
    return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255, 1];
  }
  const rgb = /^rgba\((\d+), (\d+), (\d+), ([\d.]+)\)$/.exec(value);
  if (rgb)
    return [Number(rgb[1]) / 255, Number(rgb[2]) / 255, Number(rgb[3]) / 255, Number(rgb[4])];
  return undefined;
}

/** One direction's colours in one mode: the light block, then the dark block over it. */
function palette(id: string, mode: string) {
  const sel = themeSelectors(id, shipped);
  const light = css.get(sel.light) ?? new Map<string, string>();
  const none = new Map<string, string>();
  const dark = mode === "dark" ? (css.get(sel.dark) ?? none) : none;
  const colour = (name: string, depth = 0): Rgba | undefined => {
    const alias = /^var\((--[a-z0-9-]+)\)$/.exec(aliases.get(name) ?? "");
    if (alias && depth < 10) return colour(alias[1], depth + 1);
    return parseColor(dark.get(name) ?? light.get(name) ?? "");
  };
  const surface = (name: string, depth = 0): Rgb | undefined => {
    const c = colour(name);
    if (!c) return undefined;
    if (c[3] >= 1 || depth > 5) return [c[0], c[1], c[2]];
    const under: Rgb | undefined =
      name === "--background" ? [1, 1, 1] : surface("--background", depth + 1);
    return under && composite(c, under);
  };
  return { colour, surface };
}

function measure(id: string, mode: string, p: Pair) {
  const { colour, surface } = palette(id, mode);
  const bgc = colour(p.bg);
  const under = surface(p.over ?? "--background");
  const fgc = colour(p.fg);
  if (!bgc || !under || !fgc) return undefined;
  const bg: Rgb = bgc[3] < 1 ? composite(bgc, under) : [bgc[0], bgc[1], bgc[2]];
  const fg: Rgb = fgc[3] < 1 ? composite(fgc, bg) : [fgc[0], fgc[1], fgc[2]];
  return { ratio: wcagRatio(fg, bg), lc: apcaLc(fg, bg) };
}

describe("contrast maths", () => {
  // Reference values the design validator's self-test uses (WCAG definition; apca-w3 README).
  const grey = (v: number): Rgb => [v / 255, v / 255, v / 255];
  it("matches the WCAG and APCA references", () => {
    expect(wcagRatio([0, 0, 0], [1, 1, 1])).toBeCloseTo(21, 9);
    expect(wcagRatio(grey(0x76), [1, 1, 1])).toBeGreaterThanOrEqual(4.5);
    expect(wcagRatio(grey(0x77), [1, 1, 1])).toBeLessThan(4.5);
    expect(apcaLc(grey(0x88), [1, 1, 1])).toBeCloseTo(63.056469930209424, 2);
    expect(apcaLc([1, 1, 1], grey(0x88))).toBeCloseTo(-68.54146436644962, 2);
    expect(apcaLc([0, 0, 0], grey(0xaa))).toBeCloseTo(58.146262578561334, 2);
    expect(apcaLc(grey(0xaa), [0, 0, 0])).toBeCloseTo(-56.24113336839742, 2);
  });
});

for (const { id } of THEMES) {
  for (const mode of ["light", "dark"]) {
    describe(`${id} ${mode}`, () => {
      for (const p of pairs.filter((x) => !x.modes || x.modes.includes(mode))) {
        const label = caseLabel(id, mode, p);
        const m = measure(id, mode, p);
        const reading = m
          ? `WCAG ${m.ratio.toFixed(2)}, APCA Lc ${m.lc.toFixed(1)}`
          : "a colour is missing";
        const run = KNOWN_FAILURES.includes(label) ? it.fails : it;
        run(`${label} >= ${p.min} (${reading})`, () => {
          expect(m, `${label}: a colour is missing`).toBeDefined();
          expect((m?.ratio ?? 0) + 1e-9, label).toBeGreaterThanOrEqual(p.min);
        });
      }
    });
  }
}

// The phone kit's tones with the theme, on the grounds the kit itself draws them on. The kit holds
// each tone's theme pair in its tone values (shell.css `.oxy-phone-tone--*`), and its card, row
// and KPI tile sit on the foreground mixed into the page, which is no theme token. So the pairs and
// the grounds are both read out of shell.css, and measured where the kit draws them.
const shellCss = readFileSync(join(here, "..", "shell", "shell.css"), "utf8").replace(
  /\/\*[\s\S]*?\*\//g,
  ""
);
/** The declarations of the rule whose whole selector, from the start of its line, is `selector`. */
function shellRule(selector: string): string {
  const esc = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return new RegExp(`(?:^|\\n)${esc}\\s*\\{([^}]*)\\}`).exec(shellCss)?.[1] ?? "";
}
/** The theme name a `var(...)` reads, through the shell's host token (`--oxy-shell-muted-fg` reads
 *  `--muted-foreground`) where it is one. */
function themeName(expr: string): string | undefined {
  const name = /^var\((--[a-z0-9-]+)(?:,[^)]*)?\)$/.exec(expr.trim())?.[1];
  if (!name?.startsWith("--oxy-shell-")) return name;
  const host = new RegExp(`${name}: var\\((--[a-z0-9-]+)`).exec(shellRule(".oxy-shell-scope"));
  return host?.[1];
}
function toneValue(part: string, selector: string): string | undefined {
  const m = new RegExp(`--oxy-phone-tone-${part}: ([^;]+);`).exec(shellRule(selector));
  return m ? themeName(m[1]) : undefined;
}
/** The kit surface: `color-mix(in srgb, <fg> N%, <bg>)` on its card, row and KPI tile alike. */
function kitSurfaceShare(): number | undefined {
  const re =
    /background: color-mix\(in srgb, var\(--oxy-shell-foreground\) (\d+)%, var\(--oxy-shell-background\)\);/;
  const shares = [".oxy-phone-card", ".oxy-phone-row", ".oxy-phone-kpi"].map(
    (s) => re.exec(shellRule(s))?.[1]
  );
  return shares.every((s) => s !== undefined && s === shares[0])
    ? Number(shares[0]) / 100
    : undefined;
}
const KIT_TONES = ["good", "warn", "bad", "neutral"] as const;
/** The KPI delta draws the tone's caption ink with the theme: `--oxy-phone-tone-text`, switched. */
function kpiDeltaReadsToneText(): boolean {
  return /var\(--oxy-phone-tone-text, transparent\) calc\(var\(--oxy-shell-themed, 0\) \* 100%\)/.test(
    shellRule(".oxy-phone-kpi__delta")
  );
}

for (const { id } of THEMES) {
  for (const mode of ["light", "dark"]) {
    describe(`${id} ${mode}: the phone kit's tones on the kit's own grounds`, () => {
      const { colour, surface } = palette(id, mode);
      const opaque = (name: string | undefined): Rgb | undefined =>
        name ? surface(name) : undefined;
      const share = kitSurfaceShare();
      const fg = opaque("--foreground");
      const bg = opaque("--background");
      const kit: Rgb | undefined =
        share !== undefined && fg && bg
          ? ([0, 1, 2].map((i) => fg[i] * share + bg[i] * (1 - share)) as Rgb)
          : undefined;
      const grounds: Record<string, Rgb | undefined> = {
        "the kit surface": kit,
        "the sheet": opaque(
          themeName(/background: ([^;]+);/.exec(shellRule(".oxy-phone-sheet"))?.[1] ?? "")
        ),
        "--card": opaque("--card"),
        "the tab bar": opaque(
          /background: var\((--[a-z0-9-]+),/.exec(shellRule(".oxy-phone-tabbar"))?.[1]
        )
      };
      const cases: { what: string; fg?: string; fill?: string; ground: string; min: number }[] = [];
      for (const tone of KIT_TONES) {
        const sel = `.oxy-phone-tone--${tone}`;
        for (const ground of ["the kit surface", "the sheet", "--card"])
          cases.push({
            what: `pill ${tone}`,
            fg: toneValue("ink", sel),
            fill: toneValue("fill", sel),
            ground,
            min: 4.5
          });
        cases.push({
          what: `row dot ${tone}`,
          fg: toneValue("dot", sel),
          ground: "the kit surface",
          min: 3
        });
        cases.push({
          what: `caption ${tone}`,
          fg: toneValue("text", sel),
          ground: "the kit surface",
          min: 4.5
        });
        // The KPI tile's delta (`PhoneKpi.tone`, `warn` included): the caption ink, read through
        // the delta's own rule, on the tile, which is the kit surface (`kitSurfaceShare`).
        cases.push({
          what: `kpi delta ${tone}`,
          fg: kpiDeltaReadsToneText() ? toneValue("text", sel) : undefined,
          ground: "the kit surface",
          min: 4.5
        });
        // A toned tab count: its tone class, then the count's own neutral pair over it.
        const count = `.oxy-phone-tabbar__badge--toned.oxy-phone-tone--${tone}`;
        const ink = toneValue("ink", count) ?? toneValue("ink", sel);
        const fill = toneValue("fill", count) ?? toneValue("fill", sel);
        for (const ground of ["the tab bar", "the sheet"])
          cases.push({ what: `tab count ${tone}`, fg: ink, fill, ground, min: 4.5 });
      }
      for (const c of cases) {
        const under = grounds[c.ground];
        const fillc = c.fill ? colour(c.fill) : undefined;
        const fgc = c.fg ? colour(c.fg) : undefined;
        const base: Rgb | undefined = !under
          ? undefined
          : !c.fill
            ? under
            : fillc && composite(fillc, under);
        const ink: Rgb | undefined = fgc && base && composite(fgc, base);
        const ratio = ink && base ? wcagRatio(ink, base) : undefined;
        const label = `${c.what}: ${c.fg} ${c.fill ? `on ${c.fill} ` : ""}over ${c.ground}`;
        it(`${label} >= ${c.min} (${ratio ? `WCAG ${ratio.toFixed(2)}` : "a colour is missing"})`, () => {
          expect(ratio, `${label}: a colour or ground is missing`).toBeDefined();
          expect((ratio ?? 0) + 1e-9, label).toBeGreaterThanOrEqual(c.min);
        });
      }
    });
  }
}

it("lists only known failures that exist", () => {
  const labels = new Set(
    THEMES.flatMap(({ id }) =>
      ["light", "dark"].flatMap((mode) => pairs.map((p) => caseLabel(id, mode, p)))
    )
  );
  for (const k of KNOWN_FAILURES) expect(labels, k).toContain(k);
});
