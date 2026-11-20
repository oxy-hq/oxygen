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

import { readFileSync } from "node:fs";
import { isAbsolute, resolve } from "node:path";
import contractJson from "../theme/contract/contract.json";

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
}

/** Every custom property theme.css (and motion.css, for the motion names) defines. */
export const SDK_THEME_NAMES: ReadonlySet<string> = new Set(
  (contractJson as { tokens: { name: string }[] }).tokens.map((t) => t.name)
);

const COLOUR =
  /#[0-9a-fA-F]{3,8}\b|\b(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch|color)\(|(?<![\w-])(?:white|black)(?![\w-])/i;
const MODE_CLASS = /\.(dark|light)(?![\w-])/g;
const ROOT_COMPOUND = /^(?::root|html)(?![\w-])/i;

/** Comments and the insides of strings and unquoted url()s replaced by spaces, lines kept. */
function blank(css: string): string {
  const keepLines = (s: string) => s.replace(/[^\n]/g, " ");
  return css
    .replace(/\/\*[\s\S]*?\*\//g, keepLines)
    .replace(/(["'])(?:\\.|(?!\1)[^\\\n])*\1/g, (m) => `${m[0]}${keepLines(m.slice(1, -1))}${m[0]}`)
    .replace(/url\(\s*([^"')\s][^)]*)\)/gi, (m, inner: string) =>
      m.replace(inner, keepLines(inner))
    );
}

/** The selectors of a selector list, split at top-level commas. */
function selectorList(prelude: string): { text: string; offset: number }[] {
  const out: { text: string; offset: number }[] = [];
  let depth = 0;
  let start = 0;
  for (let i = 0; i <= prelude.length; i += 1) {
    const c = prelude[i];
    if (c === "(" || c === "[") depth += 1;
    else if (c === ")" || c === "]") depth -= 1;
    if ((c === "," && depth === 0) || i === prelude.length) {
      out.push({ text: prelude.slice(start, i), offset: start });
      start = i + 1;
    }
  }
  return out;
}

/** The compound selector that holds position `at` (walking out of any `:not(…)` it sits in). */
function compoundAt(selector: string, at: number): string {
  let depth = 0;
  let i = at - 1;
  for (; i >= 0; i -= 1) {
    const c = selector[i];
    if (c === ")" || c === "]") depth += 1;
    else if (c === "(" || c === "[") {
      if (depth > 0) depth -= 1;
    } else if (depth === 0 && /[\s>+~]/.test(c)) break;
  }
  return selector.slice(i + 1).trimStart();
}

interface Scan {
  file: string;
  /** The line the scanned text starts on (a `<style>` block inside HTML). */
  firstLine: number;
  text: string;
}

function scan(src: Scan, names: ReadonlySet<string>, out: ThemeFinding[]): void {
  const text = blank(src.text);
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
  let start = 0;
  const inTheme = () => stack.some((p) => /^@theme\b/.test(p));
  const inCustomVariant = () => stack.some((p) => /^@custom-variant\b/.test(p));

  const declaration = (segment: string, at: number) => {
    const colon = segment.indexOf(":");
    if (colon < 0 || stack.length === 0) return;
    const name = segment.slice(0, colon).trim();
    const where = at + segment.indexOf(name);
    if (name.toLowerCase() === "color-scheme") {
      add("own-color-scheme", where, name, "sets color-scheme; the theme sets it per mode");
    }
    if (!name.startsWith("--")) return;
    if (inTheme()) {
      const colour = COLOUR.exec(segment.slice(colon + 1));
      if (colour)
        add(
          "literal-colour-in-theme",
          where,
          name,
          `${name} pins the literal colour ${colour[0]} in @theme; point it at a theme token (var(--…))`
        );
    } else if (names.has(name)) {
      add(
        "restates-sdk-token",
        where,
        name,
        `restates ${name}, which theme.css defines; remove it`
      );
    }
  };

  const selector = (prelude: string, at: number) => {
    if (inCustomVariant()) return;
    if (/\[\s*data-(theme|oxy-mode)\b/.test(prelude))
      add(
        "own-mode-block",
        at,
        prelude.trim(),
        "keys on its own mode attribute; the SDK owns light and dark (follow `.dark` on :root)"
      );
    for (const sel of selectorList(prelude)) {
      for (const m of sel.text.matchAll(MODE_CLASS)) {
        const compound = compoundAt(sel.text, m.index ?? 0);
        if (ROOT_COMPOUND.test(compound)) continue;
        add(
          "bare-mode-class",
          at + sel.offset + (m.index ?? 0),
          sel.text.trim(),
          `.${m[1]} is not keyed on :root or html, so any .${m[1]} ancestor matches`
        );
      }
    }
  };

  for (let i = 0; i < text.length; i += 1) {
    const c = text[i];
    if (c === "{") {
      const raw = text.slice(start, i);
      const prelude = raw.trim();
      const at = start + raw.indexOf(prelude);
      if (prelude.startsWith("@")) {
        if (/^@media\b/i.test(prelude) && /prefers-color-scheme/i.test(prelude))
          add(
            "own-mode-block",
            at,
            prelude,
            "decides light and dark itself; the SDK owns the mode"
          );
      } else if (!inTheme()) {
        selector(prelude, at);
      }
      stack.push(prelude);
      start = i + 1;
    } else if (c === "}" || c === ";") {
      declaration(text.slice(start, i), start);
      if (c === "}") stack.pop();
      start = i + 1;
    }
  }
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
    for (const s of scans) scan(s, names, out);
  }
  return out.sort((a, b) => a.file.localeCompare(b.file) || a.line - b.line);
}

/** One line per finding, `file:line rule: message`, for a readable test failure. */
export function formatThemeFindings(findings: readonly ThemeFinding[]): string {
  return findings.map((f) => `${f.at} ${f.rule}: ${f.message}`).join("\n");
}
