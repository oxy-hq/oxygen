// @vitest-environment node
//
// The desk UI's stylesheets, checked as text (as the phone kit's are, phoneCss.test.ts): every
// src/ui/**/*.css reads only names theme.css defines and paints no raw colour; ui.css, once
// built, holds every rule inside `@layer oxy-ui` exactly once; every `.oxy-ui-*` class a source
// names is styled; the touch floor is drawn and honours the shell's opt-out; and tailwind.css maps
// the reference's Tailwind names onto theme tokens with no literal value.

import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { buildUiCss, GROUPS, LAYER_ORDER, uiCssSources } from "../../scripts/build-ui-css.mjs";
import contractJson from "../theme/contract/contract.json";

const here = dirname(fileURLToPath(import.meta.url));
const CONTRACT = new Set(
  (contractJson as { tokens: { name: string }[] }).tokens.map((t) => t.name)
);

function walk(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    return statSync(path).isDirectory() ? walk(path) : [path];
  });
}

/** Comments blanked, line breaks kept, so an offset still maps to its line. */
const uncomment = (css: string) =>
  css.replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\n]/g, " "));
const lineOf = (text: string, at: number) => text.slice(0, at).split("\n").length;

const files = walk(here).filter((f) => f.endsWith(".css"));
const ruleFiles = files.filter((f) => !f.endsWith("tailwind.css"));
const css = new Map(ruleFiles.map((f) => [relative(here, f), uncomment(readFileSync(f, "utf8"))]));
const base = css.get("base.css") ?? "";

/** Every `--oxy-ui-*` property a ui stylesheet declares, with its value. */
const ownProps = new Map<string, string>();
for (const text of css.values())
  for (const m of text.matchAll(/(?<![\w-])(--oxy-ui-[a-z0-9-]+)\s*:\s*([^;}]+)/g))
    ownProps.set(m[1], m[2]);

/** A name is defined when theme.css (the contract) has it, or it is an own property whose value
 *  reads only defined names. */
function defined(name: string, seen = new Set<string>()): boolean {
  if (CONTRACT.has(name)) return true;
  const value = ownProps.get(name);
  if (value === undefined || seen.has(name)) return false;
  seen.add(name);
  return [...value.matchAll(/var\(\s*(--[a-z0-9-]+)/g)].every((m) => defined(m[1], seen));
}

/** Every `selector { declarations }` block at any depth, with the at-rules around it. */
function rules(text: string): { selector: string; body: string; at: number; within: string[] }[] {
  const out: { selector: string; body: string; at: number; within: string[] }[] = [];
  const stack: string[] = [];
  let start = 0;
  for (let i = 0; i < text.length; i += 1) {
    const c = text[i];
    if (c === "{") {
      stack.push(text.slice(start, i).trim());
      start = i + 1;
    } else if (c === "}") {
      const prelude = stack.pop() ?? "";
      if (!prelude.startsWith("@")) {
        const open = text.lastIndexOf("{", i);
        out.push({
          selector: prelude.replace(/\s+/g, " "),
          body: text.slice(open + 1, i),
          at: open,
          within: [...stack]
        });
      }
      start = i + 1;
    } else if (c === ";" && stack.length === 0) {
      start = i + 1;
    }
  }
  return out;
}

const RAW_COLOUR =
  /#[0-9a-fA-F]{3,8}\b|\b(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch|color)\(|(?<![\w-])(?:white|black|red|green|blue|gr[ae]y|silver|yellow|orange|purple|pink|navy|teal)(?![\w-])/;

describe("src/ui stylesheets", () => {
  it("are base.css and one file per group, each in the build, none left out", () => {
    expect(GROUPS).toHaveLength(10);
    const listed = uiCssSources();
    expect(listed[0]).toBe("base.css");
    expect([...css.keys()].sort()).toEqual([...listed].sort());
  });

  it("read only names theme.css defines, or --oxy-ui-* properties that resolve to them", () => {
    const unknown: string[] = [];
    for (const [file, text] of css)
      for (const m of text.matchAll(/var\(\s*(--[a-zA-Z0-9-]+)/g))
        if (!defined(m[1])) unknown.push(`${file}:${lineOf(text, m.index ?? 0)} ${m[1]}`);
    expect(unknown).toEqual([]);
    expect([...base.matchAll(/var\(/g)].length).toBeGreaterThan(100);
  });

  it("paint no raw colour and give no var() a fallback", () => {
    const found: string[] = [];
    for (const [file, text] of css) {
      for (const [i, line] of text.split("\n").entries()) {
        const raw = RAW_COLOUR.exec(line);
        if (raw) found.push(`${file}:${i + 1} ${raw[0]}`);
        const fallback = /var\(\s*--[a-zA-Z0-9-]+\s*,/.exec(line);
        if (fallback) found.push(`${file}:${i + 1} fallback ${fallback[0]}`);
      }
    }
    expect(found).toEqual([]);
  });

  it("declare no theme token: they read the theme, never restate it", () => {
    const restated: string[] = [];
    for (const [file, text] of css)
      for (const m of text.matchAll(/(?<![\w-])(--[a-zA-Z0-9-]+)\s*:/g))
        if (!m[1].startsWith("--oxy-ui-"))
          restated.push(`${file}:${lineOf(text, m.index ?? 0)} ${m[1]}`);
    expect(restated).toEqual([]);
  });

  it("hold rules only: no @import, @layer or @charset in a source file", () => {
    for (const [file, text] of css) expect(text, file).not.toMatch(/@(import|layer|charset)\b/);
  });

  it("set every font size from the type scale or a type role", () => {
    const literal: string[] = [];
    // A label inside an SVG drawn at another size than its viewBox: the step times the drawing's
    // units per CSS px, which lands it on the glass at the step (TrendChart, decision 92).
    const inUnits =
      /^calc\(var\(--type-[a-z0-9-]+-size\) \* var\(--oxy-ui-[a-z0-9-]+-units-per-px\)\)$/;
    for (const [file, text] of css)
      for (const r of rules(text))
        for (const m of r.body.matchAll(/font-size:\s*([^;]+)/g)) {
          const touch = r.within.some((w) => w.includes("pointer: coarse"));
          const ok =
            /^var\(--type-[a-z0-9-]+-size\)$/.test(m[1].trim()) ||
            inUnits.test(m[1].trim()) ||
            (touch && m[1].trim() === "16px");
          if (!ok) literal.push(`${file}:${lineOf(text, r.at)} ${r.selector} ${m[1]}`);
        }
    expect(literal).toEqual([]);
  });

  it("keep every hover under @media (hover: hover), so a tap never sticks", () => {
    const bare: string[] = [];
    for (const [file, text] of css)
      for (const r of rules(text))
        if (/:hover/.test(r.selector) && !r.within.some((w) => /\(hover:\s*hover\)/.test(w)))
          bare.push(`${file}:${lineOf(text, r.at)} ${r.selector}`);
    expect(bare).toEqual([]);
  });

  it("style every .oxy-ui-* class a source names", () => {
    const sources = walk(here)
      .filter((f) => /\.tsx?$/.test(f) && !/\.test\.tsx?$/.test(f) && !f.includes("harness"))
      .map((f) => readFileSync(f, "utf8"))
      .join("\n");
    const named = new Set(
      [...sources.matchAll(/(?<![\w-])oxy-ui-[a-z0-9_-]+/g)]
        .map((m) => m[0])
        .filter((c) => !c.endsWith("-"))
    );
    expect(named.size).toBeGreaterThan(80);
    const all = [...css.values()].join("\n");
    const styled = new Set([...all.matchAll(/\.(oxy-ui-[a-z0-9_-]+)/g)].map((m) => m[1]));
    expect([...named].filter((c) => !styled.has(c))).toEqual([]);
  });
});

describe("the touch floor (decision 16)", () => {
  const coarse = rules(base).filter((r) => r.within.some((w) => w.includes("pointer: coarse")));

  it("gives every desk control 44px and every field 16px text on a coarse pointer", () => {
    const floor = coarse.find((r) => r.selector.includes(".oxy-ui-btn"));
    expect(floor?.body).toMatch(/min-height: var\(--control-h-lg\)/);
    expect(floor?.body).toMatch(/min-width: var\(--control-h-lg\)/);
    for (const c of [".oxy-ui-select", ".oxy-ui-segment", ".oxy-ui-tab", ".oxy-ui-section-link"])
      expect(floor?.selector, c).toContain(c);
    const fields = coarse.find((r) => /font-size: 16px/.test(r.body));
    expect(fields?.selector).toContain(".oxy-ui-field");
    expect(fields?.selector).toContain(".oxy-ui-select");
  });

  it('honours data-oxy-touch-targets="off" on every rule, as shell.css does', () => {
    expect(coarse.length).toBeGreaterThanOrEqual(3);
    for (const r of coarse)
      expect(r.selector.startsWith(':root:not([data-oxy-touch-targets="off"])'), r.selector).toBe(
        true
      );
  });
});

describe("dist/ui.css, built", () => {
  // Built inside each test, so a source the build refuses fails these tests, not the file.
  const build = () => {
    const built = buildUiCss();
    return { built, bare: uncomment(built).trim() };
  };

  it("states the layer order, then holds every rule inside one @layer oxy-ui block", () => {
    const { bare } = build();
    expect(bare.startsWith(LAYER_ORDER)).toBe(true);
    const body = bare.slice(LAYER_ORDER.length).trim();
    expect(body.startsWith("@layer oxy-ui {")).toBe(true);
    let depth = 0;
    let closedAt = -1;
    for (let i = body.indexOf("{"); i < body.length; i += 1) {
      if (body[i] === "{") depth += 1;
      if (body[i] === "}") depth -= 1;
      if (depth === 0) {
        closedAt = i;
        break;
      }
    }
    expect(closedAt, "the layer block closes at the end").toBe(body.length - 1);
  });

  it("carries base.css, then every group's file, each once and in order", () => {
    const { built } = build();
    const markers = [...built.matchAll(/\/\* ---- src\/ui\/(\S+) ---- \*\//g)].map((m) => m[1]);
    expect(markers).toEqual(uiCssSources());
    for (const file of css.keys())
      expect(built.includes(readFileSync(join(here, file), "utf8").trim()), file).toBe(true);
    expect(base.trim().length).toBeGreaterThan(0);
  });
});

describe("tailwind.css", () => {
  const raw = readFileSync(join(here, "tailwind.css"), "utf8");
  const tw = uncomment(raw);
  const block = /@theme inline\s*\{([\s\S]*?)\}/.exec(tw)?.[1] ?? "";
  const decls = [...block.matchAll(/(--[a-z0-9-]+)\s*:\s*([^;]+);/g)].map((m) => [
    m[1],
    m[2].trim()
  ]);
  const names = decls.map(([n]) => n);

  it("fixes the layer order first, the same order ui.css states", () => {
    expect(tw.trim().startsWith(LAYER_ORDER)).toBe(true);
  });

  it("maps 56 --color-* (the reference's 51 and five apps paint), 26 --text-* and 8 radius names, each once", () => {
    expect(new Set(names).size).toBe(names.length);
    expect(names.filter((n) => n.startsWith("--color-"))).toHaveLength(56);
    for (const n of ["sidebar", "sidebar-border", "card-foreground", "status-neutral-series"])
      expect(names, n).toContain(`--color-${n}`);
    expect(names.filter((n) => n.startsWith("--text-"))).toHaveLength(26);
    expect(names.filter((n) => n.startsWith("--radius-")).sort()).toEqual(
      ["xs", "sm", "md", "lg", "xl", "2xl", "3xl", "4xl"].map((s) => `--radius-${s}`).sort()
    );
    expect(names).toHaveLength(56 + 26 + 8);
  });

  it("holds no literal value: every name is a var() of a theme token", () => {
    for (const [name, value] of decls) {
      const target = /^var\((--[a-z0-9-]+)\)$/.exec(value)?.[1];
      expect(target, `${name}: ${value}`).toBeDefined();
      expect(CONTRACT.has(target ?? ""), `${name} -> ${target}`).toBe(true);
    }
  });

  it("leaves the faces to theme.css: Tailwind knows --font-sans and --font-mono and declares neither", () => {
    // `reference` keeps Tailwind from declaring the names on an unlayered `:root` (which would
    // outrank theme.css's and set its own default stack); each maps onto itself, theme.css's.
    const faces = /@theme inline reference\s*\{([\s\S]*?)\}/.exec(tw)?.[1] ?? "";
    const mapped = [...faces.matchAll(/(--[a-z0-9-]+)\s*:\s*([^;]+);/g)].map((m) => [
      m[1],
      m[2].trim()
    ]);
    expect(mapped).toEqual([
      ["--font-sans", "var(--font-sans)"],
      ["--font-mono", "var(--font-mono)"]
    ]);
    for (const [name] of mapped) expect(CONTRACT.has(name), name).toBe(true);
    // No other @theme block in the file declares a face (a plain or inline one would be emitted).
    const others = tw.replace(/@theme inline reference\s*\{[\s\S]*?\}/, "");
    expect(others).not.toMatch(/--font-[a-z]+\s*:/);
  });

  it("points each --color-x at --x and each --text step at its scale step, as the reference does", () => {
    for (const [name, value] of decls) {
      if (name.startsWith("--color-")) expect(value, name).toBe(`var(--${name.slice(8)})`);
      const step = /^--text-([a-z0-9]+)(--line-height)?$/.exec(name);
      if (step)
        expect(value, name).toBe(`var(--type-scale-${step[1]}-${step[2] ? "line" : "size"})`);
    }
  });
});
