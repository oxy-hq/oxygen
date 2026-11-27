/**
 * `css-digest.json`: the declarations of a build's CSS that set a literal
 * colour or a font size, for the platform's advisory design notes. Made from
 * the stylesheets the `design_system` check already read; at most 64 KB.
 *
 * ITS OWN SMALL READER. The SDK's theme guard (`@oxy-hq/sdk/theme-guard`)
 * gives this package `checkThemeCss` and the theme's names, not the scanner it
 * reads CSS with, so the walk below is this file's: comments, strings and
 * `url()`s blanked, then each rule's selector and declarations.
 *
 * "THE APP'S OWN" is decided by what a built stylesheet lets a reader tell:
 *
 * - a declaration of one of the SDK theme's names (`--card`) is the theme's,
 *   or a restatement the `design_system` check already reports: left out;
 * - Tailwind's compiled theme (`@layer theme`, and the unlayered
 *   `:root, :host` rule), its `@layer properties` and `@property` blocks and
 *   its `--tw-*` properties hold its palette and plumbing: left out;
 * - a rule that names an SDK class (`.oxy-…`) or attribute (`[data-oxy-…]`)
 *   is the SDK's own: left out;
 * - `@font-face` declares no colour and no size: left out.
 *
 * Everything else that sets a literal colour or a font size is listed, by
 * rule, in the order the stylesheet has it, until the digest is full.
 */

import { SDK_THEME_NAMES } from "@oxy-hq/sdk/theme-guard";
import type { StyleSource } from "./design-system.js";
import { cap } from "./outcome.js";

/** The largest digest, serialized. */
export const MAX_DIGEST_BYTES = 64 * 1024;
const MAX_SELECTOR_CHARS = 160;
const MAX_VALUE_CHARS = 120;

/** One rule's declarations that set a literal colour or a font size. */
export interface DigestRule {
  file: string;
  selector: string;
  /** The at-rules around it, outermost first (`@media (min-width: 40rem)`); absent at the top level. */
  within?: string[];
  /** `color: #1a1a1a`, `font-size: 11px`. */
  declarations: string[];
}

export interface CssDigest {
  rules: DigestRule[];
  /** Declarations left out because the digest was full. */
  omitted: number;
}

const COLOUR =
  /#[0-9a-fA-F]{3,8}\b|\b(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch|color)\(|(?<![\w-])(?:white|black)(?![\w-])/i;
/** Blocks whose declarations are never the app's own colours or sizes. */
const SKIPPED_BLOCK = /^@(?:layer\s+(?:theme|properties)\s*$|property\b|font-face\b)/i;
/** A rule the SDK ships: it names one of its classes or attributes. */
const SDK_RULE = /\.oxy-|\[data-oxy-/;

const blank = (text: string) => text.replace(/[^\n]/g, " ");

/** Comments replaced by spaces, every offset kept. */
function withoutComments(css: string): string {
  return css.replace(/\/\*[\s\S]*?\*\//g, blank);
}

/** String contents and unquoted `url()`s replaced by spaces too: only structure is left to walk. */
function structureOf(css: string): string {
  return css
    .replace(/(["'])(?:\\.|(?!\1)[^\\\n])*\1/g, (m) => `${m[0]}${blank(m.slice(1, -1))}${m[0]}`)
    .replace(/url\(\s*([^"')\s][^)]*)\)/gi, (m, inner: string) => m.replace(inner, blank(inner)));
}

/** The `<style>` blocks of an HTML file; a stylesheet as it is. */
function stylesOf(source: StyleSource): string[] {
  if (!/\.html?$/i.test(source.file)) return [source.css];
  return Array.from(source.css.matchAll(/<style\b[^>]*>([\s\S]*?)<\/style>/gi), (m) => m[1] ?? "");
}

/** Whether a declaration sets a literal colour or a font size. */
function kept(name: string, value: string): boolean {
  if (name.startsWith("--tw-") || SDK_THEME_NAMES.has(name)) return false;
  const property = name.toLowerCase();
  if (property === "font-size") return true;
  // The shorthand sets a size when it carries a number (`12px/1.4 Inter`); `inherit` sets none.
  if (property === "font") return /\d/.test(value);
  return COLOUR.test(value);
}

/** Whether a block is, or sits inside, one whose declarations are never the app's own. */
function isSkipped(prelude: string, stack: readonly string[]): boolean {
  return (
    stack.some((p) => SKIPPED_BLOCK.test(p)) ||
    SKIPPED_BLOCK.test(prelude) ||
    SDK_RULE.test(prelude) ||
    // Tailwind's compiled `@theme`, outside every layer.
    (stack.length === 0 && prelude.replace(/\s/g, "") === ":root,:host")
  );
}

/**
 * The rule a block with this prelude collects into, inside the open blocks of
 * `stack`; `undefined` for an at-rule and for a block the digest leaves out.
 */
function ruleOf(file: string, prelude: string, stack: readonly string[]): DigestRule | undefined {
  if (isSkipped(prelude, stack) || prelude.startsWith("@")) return undefined;
  const rule: DigestRule = { file, selector: cap(prelude, MAX_SELECTOR_CHARS), declarations: [] };
  const within = stack.filter((p) => p.startsWith("@"));
  if (within.length > 0) rule.within = within.map((p) => cap(p, MAX_SELECTOR_CHARS));
  return rule;
}

/** Every kept declaration of one stylesheet, by rule, in source order. */
function rulesOf(file: string, css: string): DigestRule[] {
  // `readable` keeps the strings `text` blanks, at the same offsets: a selector
  // and a value are quoted from it, and the braces are found in `text`.
  const readable = withoutComments(css);
  const text = structureOf(readable);
  const rules: DigestRule[] = [];
  /** The open blocks' preludes, outermost first. */
  const stack: string[] = [];
  /** The rule each open block collects into, when it is one the digest reads. */
  const collecting: (DigestRule | undefined)[] = [];
  let start = 0;
  const declaration = (from: number, to: number) => {
    const rule = collecting[collecting.length - 1];
    const segment = readable.slice(from, to);
    const colon = segment.indexOf(":");
    if (!rule || colon < 0) return;
    const name = segment.slice(0, colon).trim();
    // Judged with its strings blanked: `content: "black"` sets no colour.
    if (!name || !kept(name, text.slice(from + colon + 1, to))) return;
    const value = segment
      .slice(colon + 1)
      .trim()
      .replace(/\s+/g, " ");
    rule.declarations.push(`${name}: ${cap(value, MAX_VALUE_CHARS)}`);
  };
  for (let i = 0; i < text.length; i += 1) {
    const c = text[i];
    if (c === "{") {
      const prelude = readable.slice(start, i).trim().replace(/\s+/g, " ");
      const rule = ruleOf(file, prelude, stack);
      if (rule) rules.push(rule);
      stack.push(prelude);
      collecting.push(rule);
      start = i + 1;
    } else if (c === "}" || c === ";") {
      declaration(start, i);
      if (c === "}") {
        stack.pop();
        collecting.pop();
      }
      start = i + 1;
    }
  }
  return rules.filter((rule) => rule.declarations.length > 0);
}

/** The digest of a build's stylesheets, cut where it would pass `maxBytes` serialized. */
export function cssDigest(
  sources: readonly StyleSource[],
  maxBytes: number = MAX_DIGEST_BYTES
): CssDigest {
  const all = sources.flatMap((source) =>
    stylesOf(source).flatMap((css) => rulesOf(source.file, css))
  );
  const digest: CssDigest = { rules: [], omitted: 0 };
  // The envelope and a six-digit count of what was left out.
  let bytes = Buffer.byteLength(JSON.stringify({ rules: [], omitted: 999_999 }), "utf8");
  for (const [index, rule] of all.entries()) {
    const size = Buffer.byteLength(JSON.stringify(rule), "utf8") + 1;
    if (bytes + size > maxBytes) {
      digest.omitted = all.slice(index).reduce((n, left) => n + left.declarations.length, 0);
      break;
    }
    bytes += size;
    digest.rules.push(rule);
  }
  return digest;
}

/** The digest as the file the platform keeps. */
export function cssDigestFile(sources: readonly StyleSource[]): Uint8Array {
  return Buffer.from(JSON.stringify(cssDigest(sources)), "utf8");
}
