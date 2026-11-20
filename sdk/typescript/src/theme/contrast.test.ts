// @vitest-environment node
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import pairsJson from "./contract/pairs.json";
import { apcaLc, composite, parseColor, type Rgb, type Rgba, wcagRatio } from "./contrast-math";
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

// The shell's Ask button with the theme: the reference's secondary button (`btnCompactSecondary`,
// reference ui.ts:615), like the app buttons beside it in the top bar. Its fills, its ink, its ⌘K
// hint and its ring are read out of shell.css (the theme half of each switched colour) and measured
// on the top bar, at rest, on hover, open and focused (a focused button keeps its resting fill).
/** Every rule body whose selector, from the start of a line, is `selector`. */
function shellRules(selector: string): string[] {
  const esc = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return [...shellCss.matchAll(new RegExp(`(?:^|\\n)${esc}\\s*\\{([^}]*)\\}`, "g"))].map(
    (m) => m[1]
  );
}
/** The name `prop` reads with the theme in `selector`'s rule: a switched colour's theme half. */
function themedHalf(selector: string, prop: string): string | undefined {
  const re = new RegExp(
    `(?:^|[\\s;])${prop}: color-mix\\(\\s*in srgb,\\s*(var\\([^)]*\\)) calc\\(var\\(--oxy-shell-themed, 0\\) \\* 100%\\)`
  );
  for (const body of shellRules(selector)) {
    const m = re.exec(body);
    if (m) return themeName(m[1]);
  }
  return undefined;
}
/** The name a plain `prop: var(...)` reads in `selector`'s rule. */
function plainRead(selector: string, prop: string): string | undefined {
  for (const body of shellRules(selector)) {
    const m = new RegExp(`(?:^|[\\s;])${prop}: (var\\([^;]*\\));`).exec(body);
    if (m) return themeName(m[1]);
  }
  return undefined;
}

for (const { id } of THEMES) {
  for (const mode of ["light", "dark"]) {
    describe(`${id} ${mode}: the shell's Ask button, the reference's secondary`, () => {
      const { colour, surface } = palette(id, mode);
      const bar = surface("--topbar-bg");
      const over = (name: string | undefined, under: Rgb | undefined): Rgb | undefined => {
        const c = name ? colour(name) : undefined;
        return c && under && composite(c, under);
      };
      const fills: Record<string, string | undefined> = {
        rest: themedHalf(".oxy-askbtn", "background"),
        hover: themedHalf(".oxy-askbtn:not(.oxy-askbtn--open):hover", "background"),
        open: themedHalf(".oxy-askbtn--open", "background"),
        focused: themedHalf(".oxy-askbtn", "background")
      };
      const ink = themedHalf(".oxy-askbtn", "color");
      const hintFill = plainRead(".oxy-askbtn__kbd", "background");
      const hintInk = themedHalf(".oxy-askbtn__kbd", "color");
      const mark = plainRead(".oxy-askbtn__mark", "color");
      const ring = /var\((--focus-ring-color),/.exec(shellRule(".oxy-askbtn:focus-visible"))?.[1];

      it("reads the reference's secondary pair and the hint's chip", () => {
        expect(fills).toEqual({
          rest: "--btn-surface",
          hover: "--btn-surface-hover",
          open: "--btn-surface-hover",
          focused: "--btn-surface"
        });
        expect([ink, hintFill, hintInk, mark, ring]).toEqual([
          "--foreground",
          "--muted",
          "--muted-foreground",
          "--primary",
          "--focus-ring-color"
        ]);
      });

      const cases: { what: string; fg: Rgb | undefined; bg: Rgb | undefined; min: number }[] = [];
      for (const [state, fill] of Object.entries(fills)) {
        const ground = over(fill, bar);
        const chip = over(hintFill, ground);
        cases.push({ what: `label ${state}`, fg: over(ink, ground), bg: ground, min: 4.5 });
        cases.push({ what: `⌘K hint ${state}`, fg: over(hintInk, chip), bg: chip, min: 4.5 });
        cases.push({ what: `mark ${state}`, fg: over(mark, ground), bg: ground, min: 3 });
      }
      const rest = over(fills.rest, bar);
      cases.push({ what: "ring on the top bar", fg: over(ring, bar), bg: bar, min: 3 });
      cases.push({ what: "ring beside the fill", fg: over(ring, bar), bg: rest, min: 3 });
      for (const c of cases) {
        const ratio = c.fg && c.bg ? wcagRatio(c.fg, c.bg) : undefined;
        it(`${c.what} >= ${c.min} (${ratio ? `WCAG ${ratio.toFixed(2)}` : "a colour is missing"})`, () => {
          expect(ratio, `${c.what}: a colour is missing`).toBeDefined();
          expect((ratio ?? 0) + 1e-9, c.what).toBeGreaterThanOrEqual(c.min);
        });
      }
    });
  }
}

// The phone tab bar's current tab with the theme: the accent blue, icon and label (decided
// 18 Nov). Its colour is read out of shell.css (the theme half of the slot's switched
// colour) and measured on the bar's own ground, as the bar's rule names it: 4.5:1 for the 11px
// label, 3:1 for the icon, in both modes.
for (const { id } of THEMES) {
  for (const mode of ["light", "dark"]) {
    describe(`${id} ${mode}: the phone tab bar's current tab, the accent blue`, () => {
      const { colour, surface } = palette(id, mode);
      const ground = /background: var\((--[a-z0-9-]+),/.exec(shellRule(".oxy-phone-tabbar"))?.[1];
      const bar = ground ? surface(ground) : undefined;
      const current = themedHalf('.oxy-phone-tabbar__slot[aria-current="page"]', "color");
      const c = current ? colour(current) : undefined;
      const ink = c && bar ? composite(c, bar) : undefined;
      const ratio = ink && bar ? wcagRatio(ink, bar) : undefined;

      it("reads the theme's --link on the bar's --topbar-bg", () => {
        expect([current, ground]).toEqual(["--link", "--topbar-bg"]);
      });

      for (const [what, min] of [
        ["label, 11px text,", 4.5],
        ["icon", 3]
      ] as const) {
        it(`${what} on the tab bar >= ${min} (${ratio ? `WCAG ${ratio.toFixed(2)}` : "a colour is missing"})`, () => {
          expect(ratio, `${what}: a colour is missing`).toBeDefined();
          expect((ratio ?? 0) + 1e-9, what).toBeGreaterThanOrEqual(min);
        });
      }
    });
  }
}

// The phone drawer's appearance row (option A, 18 Nov): "Appearance" in the muted ink on the
// drawer, and the switch's icons on their fills over the track over the drawer: the selected
// button's `--btn-surface` with full ink, the others muted ink on the bare track and on
// `--btn-surface-hover`. Every name is read out of shell.css; 4.5:1 for the 14px label, 3:1 for an
// icon (WCAG 1.4.11) and for the focus ring beside the fills, in both modes.
for (const { id } of THEMES) {
  for (const mode of ["light", "dark"]) {
    describe(`${id} ${mode}: the phone drawer's appearance row`, () => {
      const { colour, surface } = palette(id, mode);
      const over = (name: string | undefined, under: Rgb | undefined): Rgb | undefined => {
        const c = name ? colour(name) : undefined;
        return c && under && composite(c, under);
      };
      const drawerName = /background: var\((--[a-z0-9-]+),/.exec(
        shellRule(".oxy-navsheet__panel")
      )?.[1];
      const drawer = drawerName ? surface(drawerName) : undefined;
      const labelInk = plainRead(".oxy-modeswitch-row__label", "color");
      const trackName = plainRead(".oxy-modeswitch-seg", "background");
      const restInk = plainRead(".oxy-modeswitch-seg__option", "color");
      const on = ".oxy-modeswitch-seg__option--on,\n.oxy-modeswitch-seg__option--on:hover";
      const hoverFill = themedHalf(".oxy-modeswitch-seg__option:hover", "background");
      const onFill = themedHalf(on, "background");
      const onInk = plainRead(on, "color");
      const ring = /var\(--focus-ring-width, 2px\)\s+var\((--focus-ring-color),/.exec(
        shellRule(".oxy-modeswitch-seg__option:focus-visible")
      )?.[1];

      it("reads the reference's secondary pair on a quiet track", () => {
        expect([drawerName, labelInk, trackName, restInk, hoverFill, onFill, onInk, ring]).toEqual([
          "--rail-bg",
          "--muted-foreground",
          "--accent",
          "--muted-foreground",
          "--btn-surface-hover",
          "--btn-surface",
          "--foreground",
          "--focus-ring-color"
        ]);
      });

      const track = over(trackName, drawer);
      const hover = over(hoverFill, track);
      const onGround = over(onFill, track);
      const cases: { what: string; fg: Rgb | undefined; bg: Rgb | undefined; min: number }[] = [
        { what: "label on the drawer", fg: over(labelInk, drawer), bg: drawer, min: 4.5 },
        { what: "icon at rest on the track", fg: over(restInk, track), bg: track, min: 3 },
        { what: "icon on hover", fg: over(restInk, hover), bg: hover, min: 3 },
        { what: "selected icon on --btn-surface", fg: over(onInk, onGround), bg: onGround, min: 3 },
        { what: "ring beside the selected fill", fg: over(ring, drawer), bg: onGround, min: 3 },
        { what: "ring on the drawer", fg: over(ring, drawer), bg: drawer, min: 3 }
      ];
      for (const c of cases) {
        const ratio = c.fg && c.bg ? wcagRatio(c.fg, c.bg) : undefined;
        it(`${c.what} >= ${c.min} (${ratio ? `WCAG ${ratio.toFixed(2)}` : "a colour is missing"})`, () => {
          expect(ratio, `${c.what}: a colour is missing`).toBeDefined();
          expect((ratio ?? 0) + 1e-9, c.what).toBeGreaterThanOrEqual(c.min);
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
