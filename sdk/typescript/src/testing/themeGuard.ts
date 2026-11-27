// `checkThemeCss(files)`: the theme guard, for an app's own tests. One guard where the apps had
// five (INVENTORY section 7: BMG's `themeSource.test.ts`, Bookkeeping's, Store Ops'
// `scripts/theme.test.mjs`, Command Center's `themeAdoption.test.ts`, Quadrogen's
// `no-hex.test.ts`), with the union of their rules:
//
//   restates-sdk-token       a theme.css name declared outside a Tailwind `@theme` block: it
//                            shadows the SDK's token in every mode (`:root { --card: #fff }`)
//   literal-colour-in-theme  a literal colour inside `@theme`: it pins a utility to one mode
//                            (`--color-background: #0a0a0a`); point at the token instead
//   bare-mode-class          `.dark` / `.light` not keyed on `:root` or `html` (`.dark .card`
//                            matches any `.dark` ancestor). `:root:not(.dark)` and Tailwind's
//                            `@custom-variant dark (&:is(.dark *))` are allowed
//   own-color-scheme         a `color-scheme` declaration: the theme sets it per mode
//   own-mode-block           `[data-theme…]`, `[data-oxy-mode…]` or `@media
//                            (prefers-color-scheme)`: the SDK owns the light/dark setting
//                            (decision 19)
//
// It reads CSS files and the `<style>` blocks of HTML files, structurally: a declaration is
// what sits between `{` (or `;`) and `;` (or `}`) inside a rule, so a selector such as
// `.btn--primary:focus-visible` is never read as a declaration of `--primary`. Comments and
// string contents are blanked first, keeping lines. Node only (it reads files).
//
// `{ built: true }` reads an app's BUILT CSS (Vite + Tailwind v4 output, minified, with the SDK's
// own stylesheets bundled inside it), which is what the platform's design-system check holds.
// Four readings are added, each a no-op on source CSS that has none of their shapes:
//
//   - a rule inside `@layer theme` (what Tailwind compiles `@theme` into) is read as inside
//     `@theme`: its custom properties restate nothing. Tailwind's default palette lives there
//     too, as literals (`--color-red-500: oklch(…)`), and an app's compiled `@theme` cannot be
//     told from it, so no literal colour is reported there either: only a real `@theme` block
//     reports one. Its selectors are still read (Tailwind writes only `:root, :host` there; a
//     hand-written `.dark .x` in that layer gets no pass)
//   - an unlayered rule whose prelude is exactly `:root, :host` is read the same way. Tailwind
//     writes its compiled `@theme` there, outside every layer, when the first `@theme` it meets
//     is unlayered, which is the case for an app that imports `@oxy-hq/sdk/tailwind.css` before
//     `tailwindcss`, as that file says to. Without this an app's own `@theme` alias
//     (`--color-card: var(--card)`), which the source reading allows, would be reported once
//     built. A hand-written `:root, :host` rule cannot be told from Tailwind's; `:root` alone,
//     `:root, :host, .x`, and the same rule inside any other layer get no pass
//   - a rule whose prelude is one of the SDK's own is skipped: its selector, and each
//     declaration the SDK makes there (themeGuardSdkRules.gen.json, from the SDK's stylesheets)
//   - a selector is read with its CSS escapes, so `.dark\:bg-card` is no `.dark`, and the
//     `:is(.dark *)` a Tailwind variant class carries is the compiled
//     `@custom-variant dark (&:is(.dark *))`, which is allowed (themeGuardSelectors.ts)

import { readFileSync } from "node:fs";
import { isAbsolute, resolve } from "node:path";
import contractJson from "../theme/contract/contract.json";
import { blankComments, blankStrings, preludeKey, propertyKey, walk } from "./themeGuardScan";
import sdkRulesJson from "./themeGuardSdkRules.gen.json";
import { BUILT_SELECTORS, SOURCE_SELECTORS } from "./themeGuardSelectors";

export type ThemeGuardRule =
  | "restates-sdk-token"
  | "literal-colour-in-theme"
  | "bare-mode-class"
  | "own-color-scheme"
  | "own-mode-block";

export interface ThemeFinding {
  /** The file as it was given. */
  file: string;
  /** 1-based line in that file. */
  line: number;
  /** `file:line`. */
  at: string;
  rule: ThemeGuardRule;
  /** The custom property, selector or at-rule the finding is about. */
  subject: string;
  message: string;
}

/** CSS (or HTML) held in memory, named for the findings. */
export interface ThemeCssSource {
  file: string;
  css: string;
}

export interface CheckThemeCssOptions {
  /** Where relative paths resolve. Default: `process.cwd()`. */
  cwd?: string;
  /** The names the SDK theme owns. Default: every name in the theme contract. */
  names?: Iterable<string>;
  /**
   * Read the sources as an app's built CSS (Vite + Tailwind v4 output, with the SDK's own CSS
   * bundled in) instead of its source: `@layer theme` and an unlayered `:root, :host` rule read
   * as `@theme`, the SDK's own rules are skipped, and selectors are read with their CSS escapes.
   * Default: false.
   */
  built?: boolean;
}

/** Every custom property theme.css (and motion.css, for the motion names) defines. */
export const SDK_THEME_NAMES: ReadonlySet<string> = new Set(
  (contractJson as { tokens: { name: string }[] }).tokens.map((t) => t.name)
);

const COLOUR =
  /#[0-9a-fA-F]{3,8}\b|\b(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch|color)\(|(?<![\w-])(?:white|black)(?![\w-])/i;
const ROOT_COMPOUND = /^(?::root|html)(?![\w-])/i;

/**
 * The properties the SDK declares under each of its own preludes (as `preludeKey`), for every
 * style rule in theme.css, motion.css, shell.css and ui.css. Generated, and pinned to those
 * stylesheets by themeGuardBuilt.test.ts: `pnpm gen:theme-guard-rules`.
 */
let sdkRules: Map<string, ReadonlySet<string>> | undefined;

/** The properties the SDK declares under this prelude (as `preludeKey`), when it has a rule with it. */
function sdkRule(key: string): ReadonlySet<string> | undefined {
  if (!sdkRules) {
    sdkRules = new Map(
      Object.entries(sdkRulesJson as Record<string, string[]>).map(([k, v]) => [k, new Set(v)])
    );
  }
  return sdkRules.get(key);
}

/** The rule Tailwind writes its compiled `@theme` in, `:root, :host`, as a `preludeKey`. */
const COMPILED_THEME_RULE = ":root,:host";

/** What an open block is in built CSS, beside its prelude. Empty when the source is read. */
interface BuiltFrame {
  /** The SDK's own rule: the properties the SDK declares there. */
  sdk?: ReadonlySet<string>;
  /** Tailwind's compiled `@theme` outside every layer: a `:root, :host` rule, unlayered. */
  compiledTheme?: boolean;
}

interface Scan {
  file: string;
  /** The line the scanned text starts on (a `<style>` block inside HTML). */
  firstLine: number;
  text: string;
}

function scan(src: Scan, names: ReadonlySet<string>, built: boolean, out: ThemeFinding[]): void {
  // `comments` keeps the strings `text` blanks, at the same offsets: an SDK prelude is matched
  // on them (`[data-oxy-mode="dark"]`).
  const comments = blankComments(src.text);
  const text = blankStrings(comments);
  const lineStarts = [0];
  for (let i = 0; i < text.length; i += 1) if (text[i] === "\n") lineStarts.push(i + 1);
  const lineOf = (at: number) => {
    let lo = 0;
    let hi = lineStarts.length - 1;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (lineStarts[mid] <= at) lo = mid;
      else hi = mid - 1;
    }
    return src.firstLine + lo;
  };
  const add = (rule: ThemeGuardRule, at: number, subject: string, message: string) => {
    const line = lineOf(at);
    out.push({ file: src.file, line, at: `${src.file}:${line}`, rule, subject, message });
  };

  const stack: string[] = [];
  /** Beside `stack`: what each open block is in built CSS. */
  const builtStack: BuiltFrame[] = [];
  const selectors = built ? BUILT_SELECTORS : SOURCE_SELECTORS;
  const inTheme = () => stack.some((p) => /^@theme\b/.test(p));
  /** Built CSS only: where Tailwind compiles `@theme` to, `@layer theme` or its unlayered rule. */
  const inCompiledTheme = () =>
    built &&
    (stack.some((p) => /^@layer\s+theme$/.test(p)) ||
      builtStack[builtStack.length - 1]?.compiledTheme === true);
  const inCustomVariant = () => stack.some((p) => /^@custom-variant\b/.test(p));

  /** A custom property: a literal colour pinned in `@theme`, or an SDK token restated outside it. */
  const customProperty = (name: string, value: string, where: number) => {
    if (inTheme()) {
      const colour = COLOUR.exec(value);
      if (colour)
        add(
          "literal-colour-in-theme",
          where,
          name,
          `${name} pins the literal colour ${colour[0]} in @theme; point it at a theme token (var(--…))`
        );
    } else if (names.has(name) && !inCompiledTheme()) {
      add(
        "restates-sdk-token",
        where,
        name,
        `restates ${name}, which theme.css defines; remove it`
      );
    }
  };

  const declaration = (segment: string, at: number) => {
    const colon = segment.indexOf(":");
    if (colon < 0 || stack.length === 0) return;
    const name = segment.slice(0, colon).trim();
    if (builtStack[builtStack.length - 1]?.sdk?.has(propertyKey(name))) return;
    const where = at + segment.indexOf(name);
    if (name.toLowerCase() === "color-scheme") {
      add("own-color-scheme", where, name, "sets color-scheme; the theme sets it per mode");
    }
    if (name.startsWith("--")) customProperty(name, segment.slice(colon + 1), where);
  };

  /** A selector's mode classes that are neither keyed on the root nor a Tailwind variant class's own. */
  const bareModeClasses = (sel: string) =>
    selectors
      .modeClasses(sel)
      .filter(
        (m) =>
          !ROOT_COMPOUND.test(selectors.compoundAt(sel, m.index)) &&
          !selectors.isVariantClass(sel, m.index, m.mode)
      );

  const selector = (prelude: string, at: number) => {
    if (inCustomVariant()) return;
    if (/\[\s*data-(theme|oxy-mode)\b/.test(prelude))
      add(
        "own-mode-block",
        at,
        prelude.trim(),
        "keys on its own mode attribute; the SDK owns light and dark (follow `.dark` on :root)"
      );
    for (const sel of selectors.list(prelude)) {
      for (const m of bareModeClasses(sel.text)) {
        add(
          "bare-mode-class",
          at + sel.offset + m.index,
          sel.text.trim(),
          `.${m.mode} is not keyed on :root or html, so any .${m.mode} ancestor matches`
        );
      }
    }
  };

  walk(text, {
    open(prelude, at) {
      const frame: BuiltFrame = {};
      if (prelude.startsWith("@")) {
        if (/^@media\b/i.test(prelude) && /prefers-color-scheme/i.test(prelude))
          add(
            "own-mode-block",
            at,
            prelude,
            "decides light and dark itself; the SDK owns the mode"
          );
      } else {
        if (built) {
          const key = preludeKey(comments.slice(at, at + prelude.length));
          frame.sdk = sdkRule(key);
          // Top level only: Tailwind writes it under no at-rule at all, so the same
          // rule inside a layer, an `@media` or an `@supports` is the app's own.
          frame.compiledTheme = key === COMPILED_THEME_RULE && stack.length === 0;
        }
        if (!frame.sdk && !inTheme()) selector(prelude, at);
      }
      stack.push(prelude);
      builtStack.push(frame);
    },
    segment: declaration,
    close() {
      stack.pop();
      builtStack.pop();
    }
  });
}

/** The `<style>` blocks of an HTML file, each with the line it starts on. */
function styleBlocks(file: string, html: string): Scan[] {
  const out: Scan[] = [];
  for (const m of html.matchAll(/<style\b[^>]*>([\s\S]*?)<\/style>/gi)) {
    const bodyAt = (m.index ?? 0) + m[0].indexOf(">") + 1;
    out.push({ file, firstLine: html.slice(0, bodyAt).split("\n").length, text: m[1] });
  }
  return out;
}

/**
 * Every place an app's CSS (or an HTML file's `<style>`) works against the SDK theme, as
 * `file:line` findings; `[]` when there are none. Give it the app's stylesheets and its
 * `index.html`:
 *
 *   expect(checkThemeCss(["src/index.css", "index.html"])).toEqual([]);
 *
 * For an app's built CSS (a bundle's `index.html` and its `assets/*.css`), pass `built`:
 *
 *   checkThemeCss([{ file: "assets/index-abc.css", css }], { built: true });
 */
export function checkThemeCss(
  files: Iterable<string | ThemeCssSource>,
  options: CheckThemeCssOptions = {}
): ThemeFinding[] {
  const names = options.names ? new Set(options.names) : SDK_THEME_NAMES;
  const cwd = options.cwd ?? process.cwd();
  const out: ThemeFinding[] = [];
  for (const f of files) {
    const file = typeof f === "string" ? f : f.file;
    const text =
      typeof f === "string" ? readFileSync(isAbsolute(f) ? f : resolve(cwd, f), "utf8") : f.css;
    const scans = /\.html?$/i.test(file) ? styleBlocks(file, text) : [{ file, firstLine: 1, text }];
    for (const s of scans) scan(s, names, options.built === true, out);
  }
  return out.sort((a, b) => a.file.localeCompare(b.file) || a.line - b.line);
}

/** One line per finding, `file:line rule: message`, for a readable test failure. */
export function formatThemeFindings(findings: readonly ThemeFinding[]): string {
  return findings.map((f) => `${f.at} ${f.rule}: ${f.message}`).join("\n");
}
