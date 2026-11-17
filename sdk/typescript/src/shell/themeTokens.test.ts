// @vitest-environment node
// How shell.css reads the theme, checked as text. The shell and phone kit read the theme's names
// first and what they drew before second, each where it is used, so an app that does not import
// `@oxy-hq/sdk/theme.css` renders exactly as it did; an app that does gets the reference app's
// corners, frame lines and touch heights. The pixel half of that claim is a render comparison,
// not a unit test (see the PR); this holds the shape that makes it true.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import contractJson from "../theme/contract/contract.json";
import { cssBlockMap, themeSelectors } from "../theme/css-blocks";

const here = dirname(fileURLToPath(import.meta.url));
const css = readFileSync(join(here, "shell.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
const theme = cssBlockMap(readFileSync(join(here, "..", "theme", "theme.css"), "utf8"));
const shipped = theme.get(themeSelectors("reference", "reference").light) ?? new Map();
const contract = new Set(
  (contractJson as { tokens: { name: string }[] }).tokens.map((t) => t.name)
);

/** Names an app sets on its own today (Tailwind's theme, shadcn, the apps' palettes). The shell
 *  already reads some of them as host tokens; reading any more of them first would move an app
 *  that never imported the theme. */
const HOST = new Set([
  "--background",
  "--foreground",
  "--card",
  "--muted",
  "--muted-foreground",
  "--primary",
  "--primary-foreground",
  "--accent",
  "--border",
  "--border-strong",
  "--border-subtle",
  "--ring",
  "--success",
  "--destructive",
  "--info",
  "--radius",
  "--font-sans",
  "--font-inter",
  "--font-mono",
  "--sidebar-background",
  "--sidebar-accent",
  "--sidebar-accent-foreground",
  "--sidebar-border",
  "--zinc-200",
  "--input",
  "--field-border",
  // Set by apps on their own too: Command Center (`--secondary`, styles.css), Store Ops
  // (`--secondary`, `--segment-track`, `--segment-selected`, `--btn-surface*`, index.css).
  "--secondary",
  "--secondary-foreground",
  "--segment-track",
  "--segment-selected",
  "--btn-surface",
  "--btn-surface-hover",
  // Store Ops sets the status pairs and the dim ink (index.css on pokehouse main); Command
  // Center set the status dots (styles.css on PR #564).
  "--status-success",
  "--status-success-bg",
  "--status-warning",
  "--status-warning-bg",
  "--status-error",
  "--status-error-bg",
  "--status-neutral-bg",
  "--dim-foreground",
  "--dot-success",
  "--dot-warning",
  "--dot-error"
]);

/** The host names 2.28's shell.css already read (as its `--oxy-shell-*` host tokens, and the
 *  kit's `--font-sans`). Any other host name may be read only behind `--oxy-shell-themed`. */
const HOST_READ_IN_2_28 = new Set([
  "--sidebar-background",
  "--sidebar-accent",
  "--sidebar-accent-foreground",
  "--sidebar-border",
  "--background",
  "--foreground",
  "--muted",
  "--muted-foreground",
  "--primary",
  "--primary-foreground",
  "--accent",
  "--border",
  "--border-strong",
  "--ring",
  "--success",
  "--destructive",
  "--zinc-200",
  "--info",
  "--radius",
  "--font-inter",
  "--font-sans"
]);

/** Every rule as [selector, body]; rules inside @media and @layer blocks included. */
function rules(): [string, string][] {
  return [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)].map((m) => [m[1].trim(), m[2]]);
}

/** A theme-switched colour: all `name` where the theme is loaded, the 2.28 value where not. */
const switched = (name: string) =>
  new RegExp(
    `color-mix\\(\\s*in srgb,\\s*var\\(${name}, transparent\\) calc\\(var\\(--oxy-shell-themed, 0\\) \\* 100%\\),`,
    "g"
  );

/** The kit's theme-only tone values: each tone class holds its theme pair in them, and the kit's
 *  pill, dot, caption and tab count read them only behind the switch. */
const TONE_VALUES = [
  "--oxy-phone-tone-fill",
  "--oxy-phone-tone-ink",
  "--oxy-phone-tone-text",
  "--oxy-phone-tone-dot"
];
/** A theme name held in a tone value: `--oxy-phone-tone-<part>: var(<name>, transparent);`. */
const toneValue = (name: string) =>
  new RegExp(`--oxy-phone-tone-(?:fill|ink|text|dot): var\\(${name}, transparent\\);`, "g");

/** Every `var(--name, fallback)` and bare `var(--name)` the shell reads, by name. */
function reads(): { name: string; fallback: boolean }[] {
  return [...css.matchAll(/var\((--[a-z0-9-]+)\s*(,)?/g)].map((m) => ({
    name: m[1],
    fallback: m[2] === ","
  }));
}
const themeReads = () =>
  reads().filter((r) => contract.has(r.name) && !HOST.has(r.name) && !r.name.startsWith("--oxy-"));

/** The declarations of the first rule whose selector list is exactly `selector`. */
function rule(selector: string): string {
  const esc = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const m = new RegExp(`(?:^|[}\\s])${esc}\\s*\\{([^}]*)\\}`).exec(css);
  return m ? m[1] : "";
}

describe("shell.css reads the theme", () => {
  it("reads theme names, and every one is in the contract and shipped in theme.css", () => {
    const names = [...new Set(themeReads().map((r) => r.name))].sort();
    expect(names).toEqual(
      expect.arrayContaining([
        "--badge-radius",
        "--button-primary-bg",
        "--button-primary-fg",
        "--button-radius",
        "--card-border",
        "--card-radius",
        "--control-h-lg",
        "--focus-ring-color",
        "--focus-ring-width",
        "--input-radius",
        "--rail-bg",
        "--rail-border",
        "--radius-control",
        "--radius-overlay",
        "--radius-pill",
        "--radius-surface",
        "--topbar-bg",
        "--topbar-border"
      ])
    );
    for (const name of names) expect(shipped.has(name), name).toBe(true);
  });

  it("falls back on every theme name to what it drew before, so an app without the theme is unmoved", () => {
    const bare = themeReads().filter((r) => !r.fallback);
    expect(bare).toEqual([]);
  });

  it("clamps every corner computed off --oxy-shell-radius at zero", () => {
    const calcs = css.match(/calc\(var\(--oxy-shell-radius\) - \d+px\)/g) ?? [];
    const clamped = css.match(/max\(0px, calc\(var\(--oxy-shell-radius\) - \d+px\)\)/g) ?? [];
    expect(calcs.length).toBeGreaterThan(20);
    expect(clamped.length).toBe(calcs.length);
  });

  it("draws frames on the frame tokens and leaves controls on --border", () => {
    expect(rule(".oxy-rail")).toMatch(
      /border-right: 1px solid var\(--rail-border, var\(--oxy-shell-border\)\)/
    );
    expect(rule(".oxy-topbar")).toMatch(
      /border-bottom: 1px solid var\(--topbar-border, var\(--oxy-shell-border\)\)/
    );
    expect(rule(".oxy-phone-card")).toMatch(
      /border: 1px solid var\(--card-border, var\(--oxy-shell-border\)\)/
    );
    expect(rule(".oxy-phone-kpi")).toMatch(
      /border: 1px solid var\(--card-border, var\(--oxy-shell-border\)\)/
    );
    // Chips, segments and secondary buttons: the reference's secondary-button edge (the frame
    // line on a filled control); a field keeps the 3:1 control line.
    expect(rule(".oxy-phone-chip")).toMatch(
      /border: 1px solid var\(--button-secondary-border, var\(--oxy-shell-border\)\)/
    );
    expect(rule(".oxy-askbtn")).toMatch(
      /border: 1px solid var\(--button-secondary-border, var\(--oxy-shell-border\)\)/
    );
    expect(rule(".oxy-phone-segmented__option--active")).toMatch(
      /inset 0 0 0 1px var\(--button-secondary-border, var\(--oxy-shell-border\)\)/
    );
    expect(rule(".oxy-askdock__composer")).toMatch(/border: 1px solid var\(--oxy-shell-border\)/);
  });

  it("takes touch heights from --control-h-lg, which the theme keeps at the 44px floor", () => {
    const touch = [...css.matchAll(/var\(--control-h-lg, ([0-9.]+)(px|rem)\)/g)];
    expect(touch.length).toBeGreaterThanOrEqual(10);
    for (const m of touch) expect(Number(m[1]) * (m[2] === "rem" ? 16 : 1)).toBe(44);
    expect(shipped.get("--control-h-lg")).toBe("44px");
  });

  it("hides the keyboard hint whenever the phone layout is on, a phone on its side included", () => {
    const at = css.indexOf(
      "@media (pointer: coarse) and (max-height: 500px) {\n  .oxy-askbtn__kbd"
    );
    expect(at).toBeGreaterThan(css.indexOf("@media (min-width: 640px)"));
    expect(css.slice(at, css.indexOf("}\n}", at))).toMatch(/\.oxy-askbtn__kbd \{\s*display: none;/);
  });
});

describe("shell.css follows the design system's own parts with the theme", () => {
  it("switches on the theme's contract marker, which only theme.css sets", () => {
    expect(rule(".oxy-shell-scope")).toMatch(
      /--oxy-shell-themed: min\(1, var\(--oxy-theme-contract, 0\)\);/
    );
    expect(shipped.get("--oxy-theme-contract")).toBe("1");
  });

  it("reads a name apps also set only where 2.28 did, or behind the theme switch", () => {
    const hostReads = reads().filter((r) => HOST.has(r.name) && !HOST_READ_IN_2_28.has(r.name));
    const names = [...new Set(hostReads.map((r) => r.name))].sort();
    expect(names).toEqual([
      "--dim-foreground",
      "--dot-error",
      "--dot-success",
      "--dot-warning",
      "--secondary",
      "--segment-selected",
      "--segment-track",
      "--status-error",
      "--status-error-bg",
      "--status-neutral-bg",
      "--status-success",
      "--status-success-bg",
      "--status-warning",
      "--status-warning-bg"
    ]);
    for (const name of names) {
      // Read in place behind the switch, or held in a tone value, which is itself read only
      // behind the switch (the loop after this one).
      // A read of exactly `name`: `--status-error` is not `--status-error-bg`.
      const all = css.match(new RegExp(`var\\(${name}[,)]`, "g")) ?? [];
      const held = css.match(toneValue(name))?.length ?? 0;
      expect((css.match(switched(name))?.length ?? 0) + held, name).toBe(all.length);
      expect(shipped.has(name), name).toBe(true);
    }
    for (const value of TONE_VALUES) {
      const all = css.match(new RegExp(`var\\(${value}\\b`, "g")) ?? [];
      expect(all.length, value).toBeGreaterThan(0);
      expect(css.match(switched(value))?.length ?? 0, value).toBe(all.length);
    }
  });

  /** The tone values a tone class holds, as `{ fill, ink, text, dot }` (the names they read). */
  function toneValues(tone: string) {
    const body = rule(`.oxy-phone-tone--${tone}`);
    const get = (part: string) =>
      new RegExp(`--oxy-phone-tone-${part}: var\\((--[a-z0-9-]+)(?:, transparent)?\\);`).exec(
        body
      )?.[1];
    return { fill: get("fill"), ink: get("ink"), text: get("text"), dot: get("dot") };
  }

  it("gives each kit tone the theme's status pair, its caption ink and its dot", () => {
    // The reference's `tag` + `chipTone` pair (ui.ts:458-468), its caption `note` coloured by a
    // verdict (Stat.tsx), and `StatusDot` (the neutral dot is the unjudged series grey).
    expect(toneValues("good")).toEqual({
      fill: "--status-success-bg",
      ink: "--status-success",
      text: "--status-success",
      dot: "--dot-success"
    });
    expect(toneValues("warn")).toEqual({
      fill: "--status-warning-bg",
      ink: "--status-warning",
      text: "--status-warning",
      dot: "--dot-warning"
    });
    expect(toneValues("bad")).toEqual({
      fill: "--status-error-bg",
      ink: "--status-error",
      text: "--status-error",
      dot: "--dot-error"
    });
    expect(toneValues("neutral")).toEqual({
      fill: "--status-neutral-bg",
      ink: "--dim-foreground",
      text: "--oxy-shell-muted-fg",
      dot: "--status-neutral-series"
    });
    // `--oxy-phone-tone` itself is what 2.28 set (apps read it: Site Scout's ToneText).
    expect(rule(".oxy-phone-tone--good")).toMatch(/--oxy-phone-tone: var\(--oxy-shell-success\);/);
    expect(rule(".oxy-phone-tone--warn")).toMatch(
      /--oxy-phone-tone: color-mix\(in oklch, var\(--oxy-shell-destructive\) 50%, var\(--oxy-shell-success\)\);/
    );
    expect(rule(".oxy-phone-tone--bad")).toMatch(
      /--oxy-phone-tone: var\(--oxy-shell-destructive\);/
    );
    expect(rule(".oxy-phone-tone--neutral")).toMatch(
      /--oxy-phone-tone: var\(--oxy-shell-muted-fg\);/
    );
  });

  it("draws the kit's pill, row dot and caption in the tone values with the theme, its own mix without", () => {
    const mix16 = /color-mix\(in srgb, var\(--oxy-phone-tone\) 16%, transparent\)\s*\);/;
    const ink65 =
      /color-mix\(in oklab, var\(--oxy-phone-tone\) 65%, var\(--oxy-shell-foreground\)\)\s*\);/;
    const pill = rule(".oxy-phone-pill");
    expect(pill).toMatch(
      new RegExp(`background: ${switched("--oxy-phone-tone-fill").source}\\s*${mix16.source}`)
    );
    expect(pill).toMatch(
      new RegExp(`color: ${switched("--oxy-phone-tone-ink").source}\\s*${ink65.source}`)
    );
    expect(rule(".oxy-phone-row__dot")).toMatch(
      new RegExp(
        `background: ${switched("--oxy-phone-tone-dot").source}\\s*var\\(--oxy-phone-tone\\)\\s*\\);`
      )
    );
    expect(rule(".oxy-phone-kpi__delta")).toMatch(
      new RegExp(`color: ${switched("--oxy-phone-tone-text").source}\\s*${ink65.source}`)
    );
  });

  it("draws a tab badge with no tone as 2.28 did, and a toned one as the reference's tab count", () => {
    // No tone: the shell's red with the page ink, with or without the theme.
    const plain = rule(".oxy-phone-tabbar__badge");
    expect(plain).toMatch(/background: var\(--oxy-shell-destructive\);/);
    expect(plain).toMatch(/color: var\(--oxy-shell-background\);/);
    expect(rule(".oxy-phone-tabbar__dot")).toMatch(/background: var\(--oxy-shell-destructive\);/);
    // A tone: `tabCount` on its tone's chip pair, or `tabCountNeutral` (--muted, muted ink).
    const toned = rule(".oxy-phone-tabbar__badge--toned");
    expect(toned).toMatch(
      new RegExp(
        `background: ${switched("--oxy-phone-tone-fill").source}\\s*color-mix\\(in srgb, var\\(--oxy-phone-tone\\) 16%, transparent\\)\\s*\\);`
      )
    );
    expect(toned).toMatch(new RegExp(`color: ${switched("--oxy-phone-tone-ink").source}`));
    const neutral = rule(".oxy-phone-tabbar__badge--toned.oxy-phone-tone--neutral");
    expect(neutral).toMatch(/--oxy-phone-tone-fill: var\(--oxy-shell-muted\);/);
    expect(neutral).toMatch(/--oxy-phone-tone-ink: var\(--oxy-shell-muted-fg\);/);
    expect(rule(".oxy-phone-tabbar__dot--toned")).toMatch(
      new RegExp(
        `background: ${switched("--oxy-phone-tone-dot").source}\\s*var\\(--oxy-phone-tone\\)\\s*\\);`
      )
    );
  });

  it("draws every focus ring at the theme's width and colour, at full strength", () => {
    // 2.28 drew each ring as the host ring mixed down to 45-60%; that stays the fallback, and the
    // theme's ring (1.5px of an opaque colour, the reference's `focusRing`) comes first.
    const mixed = css.match(/color-mix\(in srgb, var\(--oxy-shell-ring\) \d+%, transparent\)/g);
    const themed = css.match(
      /0 0 0 var\(--focus-ring-width, [23]px\)\s+var\(--focus-ring-color, color-mix\(in srgb, var\(--oxy-shell-ring\) \d+%, transparent\)\)/g
    );
    expect(mixed?.length).toBeGreaterThanOrEqual(24);
    expect(themed?.length).toBe(mixed?.length);
    expect(shipped.get("--focus-ring-width")).toBe("1.5px");
    expect(shipped.get("--focus-ring-color")).toMatch(/^#[0-9a-f]{6}$/);
  });

  it("gives every focus-visible control a transparent outline for forced colours", () => {
    // Forced colours drop box-shadow, so a ring drawn only as a shadow leaves no focus at all.
    // The outline is transparent, so nothing changes outside forced colours.
    const focused = rules().filter(([sel]) => /:focus-visible\s*$/m.test(sel));
    expect(focused.length).toBeGreaterThanOrEqual(24);
    for (const [sel, body] of focused) {
      expect(body, sel).toMatch(/outline: 2px solid transparent;/);
      expect(body, sel).not.toMatch(/outline: none/);
    }
  });

  it("draws the kit's primary button in the theme's primary pair, the brand fill", () => {
    const body = rule(".oxy-phone-btn--primary");
    expect(body).toMatch(/background: var\(--button-primary-bg, var\(--oxy-shell-primary\)\);/);
    expect(body).toMatch(/color: var\(--button-primary-fg, var\(--oxy-shell-primary-fg\)\);/);
    expect(shipped.get("--button-primary-bg")).toBe(shipped.get("--brand"));
  });

  it("fills the kit's secondary button with --secondary and its selected segment with --segment-selected", () => {
    expect(rule(".oxy-phone-btn--secondary")).toMatch(switched("--secondary"));
    expect(rule(".oxy-phone-btn--secondary")).toMatch(
      /color-mix\(in srgb, var\(--oxy-shell-foreground\) 4%, var\(--oxy-shell-background\)\)\s*\);/
    );
    expect(rule(".oxy-phone-segmented__option--active")).toMatch(switched("--segment-selected"));
    expect(rule(".oxy-phone-segmented__option--active")).toMatch(
      /var\(--oxy-shell-background\)\s*\);/
    );
    expect(shipped.get("--secondary")).toBe(shipped.get("--btn-surface"));
  });

  it("sets the keyboard hint at 10px without the theme and 11px with it", () => {
    const sizes = rules()
      .filter(
        ([sel, body]) =>
          sel.split(",").some((s) => s.trim() === ".oxy-askbtn__kbd") && /font-size/.test(body)
      )
      .map(([, body]) => /font-size: ([^;]+);/.exec(body)?.[1]);
    expect(sizes).toEqual(["calc(10px + var(--oxy-shell-themed, 0) * 1px)"]);
  });

  it("keeps the Today clock's whole reach on a phone with the theme", () => {
    const at = css.indexOf(
      "@media (max-width: 639.98px), (pointer: coarse) and (max-height: 500px)"
    );
    expect(at).toBeGreaterThan(0);
    expect(css.slice(at)).toMatch(
      /\.oxy-clock--button::before \{\s*z-index: var\(--oxy-shell-themed, 0\);/
    );
  });

  it("puts the kit's segment track on --segment-track with the theme, 2.28's 8% ink without", () => {
    const body = rule(".oxy-phone-segmented");
    expect(body).toMatch(switched("--segment-track"));
    expect(body).toMatch(
      /color-mix\(in srgb, var\(--oxy-shell-foreground\) 8%, var\(--oxy-shell-background\)\)\s*\);/
    );
    // The selected fill steps off the track as on the reference's desk strip.
    expect(shipped.get("--segment-track")).not.toBe(shipped.get("--segment-selected"));
  });

  it("draws the tab bar's selected label in foreground ink with the theme, the link blue without", () => {
    expect(rule('.oxy-phone-tabbar__slot[aria-current="page"]')).toMatch(
      /color: color-mix\(\s*in srgb,\s*var\(--oxy-shell-foreground\) calc\(var\(--oxy-shell-themed, 0\) \* 100%\),\s*var\(--oxy-shell-link\)\s*\);/
    );
  });

  it("draws the breadcrumb's separator in the full muted ink with the theme, 40% of it without", () => {
    expect(rule(".oxy-breadcrumb__sep")).toMatch(
      /color: color-mix\(\s*in srgb,\s*var\(--oxy-shell-muted-fg\) calc\(40% \+ 60% \* var\(--oxy-shell-themed, 0\)\),\s*transparent\s*\);/
    );
  });

  /** The body of `selector`'s rule inside each `@container <query>` block, and outside any. */
  function queried(selector: string) {
    const inside: { query: string; body: string }[] = [];
    let outside = css;
    for (const m of css.matchAll(/@container ([^{]+)\{((?:[^{}]*\{[^{}]*\})*)\s*\}/g)) {
      outside = outside.replace(m[0], "");
      for (const r of m[2].matchAll(/([^{}]+)\{([^{}]*)\}/g))
        if (r[1].trim() === selector) inside.push({ query: m[1].trim(), body: r[2] });
    }
    const esc = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    const out = [...outside.matchAll(new RegExp(`(?:^|[}\\s])${esc}\\s*\\{([^}]*)\\}`, "g"))].map(
      (m) => m[1]
    );
    return { inside, outside: out };
  }
  const THEMED_QUERY = `style(--oxy-theme-contract: ${String(contractJson.version)})`;

  it("rings the breadcrumb home link with the theme only, so without it the browser's outline stays", () => {
    const { inside, outside } = queried(".oxy-breadcrumb__workspace:focus-visible");
    expect(outside).toEqual([]);
    expect(inside.map((r) => r.query)).toEqual([THEMED_QUERY]);
    expect(inside[0].body).toMatch(/outline: 2px solid transparent;/);
    expect(inside[0].body).toMatch(
      /box-shadow: 0 0 0 var\(--focus-ring-width, 1\.5px\) var\(--focus-ring-color, var\(--oxy-shell-ring\)\);/
    );
  });

  it("draws the Today clock in the shell's sans face with the theme, monospace without", () => {
    const { inside, outside } = queried(".oxy-clock");
    expect(outside.filter((b) => /font-family/.test(b))).toEqual([
      expect.stringMatching(/font-family: var\(--oxy-shell-font-mono\);/)
    ]);
    expect(outside.join("")).toMatch(/font-variant-numeric: tabular-nums;/);
    expect(inside).toEqual([
      {
        query: THEMED_QUERY,
        body: expect.stringMatching(/^\s*font-family: var\(--oxy-shell-font\);\s*$/)
      }
    ]);
  });

  it("queries only the marker theme.css sets, at the version it sets", () => {
    const queries = [...css.matchAll(/@container ([^{]+)\{/g)].map((m) => m[1].trim());
    expect(queries.length).toBeGreaterThanOrEqual(2);
    for (const q of queries) expect(q).toBe(THEMED_QUERY);
    expect(shipped.get("--oxy-theme-contract")).toBe(String(contractJson.version));
  });

  it("draws a focused rail tile, and so its ring, at full strength with the theme", () => {
    // The tile rests at 0.6, which put the ring at 2.74:1; 0.6 stays the value without the theme.
    expect(rule(".oxy-rail__item")).toMatch(/opacity: 0\.6;/);
    expect(rule(".oxy-rail__item:focus-visible:not(:hover):not(.oxy-rail__item--active)")).toMatch(
      /opacity: calc\(0\.6 \+ 0\.4 \* var\(--oxy-shell-themed, 0\)\);/
    );
  });
});
