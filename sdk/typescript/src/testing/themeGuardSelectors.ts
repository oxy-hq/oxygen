// How the theme guard reads a selector (themeGuard.ts). Two readings:
//
//   SOURCE_SELECTORS  an app's source CSS, character by character
//   BUILT_SELECTORS   built CSS (Vite + Tailwind v4, minified), with its CSS escapes: a compiled
//                     utility is one class whatever it holds (`.dark\:bg-card` is no `.dark`,
//                     and `\[`, `\(` and the space that ends `\32 ` are no bracket and no
//                     combinator), and the `:is(.dark *)` a variant class carries is the compiled
//                     `@custom-variant dark (&:is(.dark *))`, which is allowed
//
// Source CSS rarely holds a backslash, so the built reading of it finds what the source reading
// finds.

/** One selector of a selector list, and where it starts in the prelude. */
export interface ListedSelector {
  text: string;
  offset: number;
}

/** A `.dark` or `.light` class in a selector. */
export interface ModeClass {
  index: number;
  mode: string;
}

export interface SelectorReading {
  /** The selectors of a selector list, split at top-level commas. */
  list(prelude: string): ListedSelector[];
  /** Each `.dark` / `.light` class of a selector. */
  modeClasses(selector: string): ModeClass[];
  /** The compound selector that holds position `at` (walking out of any `:not(…)` it sits in). */
  compoundAt(selector: string, at: number): string;
  /** Whether the mode class at `index` is Tailwind's compiled dark variant on a utility. */
  isVariantClass(selector: string, index: number, mode: string): boolean;
}

const MODE_CLASS = /\.(dark|light)(?![\w-])/g;
/** As MODE_CLASS, where an escape continues the name: `.dark\:bg-card` is the class `dark:bg-card`. */
const MODE_CLASS_ESCAPED = /\.(dark|light)(?![\w\\-])/g;

function backslashesBefore(text: string, i: number): number {
  let n = 0;
  while (n < i && text[i - n - 1] === "\\") n += 1;
  return n;
}

/**
 * Whether the character at `i` belongs to an escape: the one after a backslash, or the
 * whitespace that ends a hex escape (`\32 xl`, a class that starts with a digit).
 */
function isEscaped(text: string, i: number): boolean {
  if (backslashesBefore(text, i) % 2 === 1) return true;
  if (!/[ \t\n\r\f]/.test(text[i])) return false;
  let digits = 0;
  while (digits < 6 && digits < i && /[0-9a-fA-F]/.test(text[i - digits - 1])) digits += 1;
  const slash = i - digits;
  return (
    digits > 0 && slash > 0 && text[slash - 1] === "\\" && backslashesBefore(text, slash) % 2 === 1
  );
}

function selectorList(prelude: string, escapes: boolean): ListedSelector[] {
  const out: ListedSelector[] = [];
  let depth = 0;
  let start = 0;
  for (let i = 0; i <= prelude.length; i += 1) {
    const c = prelude[i];
    if (escapes && c === "\\" && i + 1 < prelude.length) {
      i += 1;
      continue;
    }
    if (c === "(" || c === "[") depth += 1;
    else if (c === ")" || c === "]") depth -= 1;
    if ((c === "," && depth === 0) || i === prelude.length) {
      out.push({ text: prelude.slice(start, i), offset: start });
      start = i + 1;
    }
  }
  return out;
}

/** Where the compound selector that holds position `at` starts. */
function compoundStart(selector: string, at: number, escapes: boolean): number {
  let depth = 0;
  let i = at - 1;
  for (; i >= 0; i -= 1) {
    if (escapes && isEscaped(selector, i)) continue;
    const c = selector[i];
    if (c === ")" || c === "]") depth += 1;
    else if (c === "(" || c === "[") {
      if (depth > 0) depth -= 1;
    } else if (depth === 0 && /[\s>+~]/.test(c)) break;
  }
  return i + 1;
}

/** Where the bracket that directly holds position `at` opens, escapes skipped; -1 when none does. */
function openerBefore(sel: string, at: number): number {
  let depth = 0;
  for (let i = at - 1; i >= 0; i -= 1) {
    if (isEscaped(sel, i)) continue;
    const c = sel[i];
    if (c === ")" || c === "]") depth += 1;
    else if (c === "(" || c === "[") {
      if (depth === 0) return i;
      depth -= 1;
    }
  }
  return -1;
}

/** Where the `(` at `open` closes, escapes skipped; -1 when it never does. */
function closerAfter(sel: string, open: number): number {
  let depth = 0;
  for (let i = open + 1; i < sel.length; i += 1) {
    if (isEscaped(sel, i)) continue;
    const c = sel[i];
    if (c === "(" || c === "[") depth += 1;
    else if (c === ")" || c === "]") {
      if (depth > 0) depth -= 1;
      else if (c === ")") return i;
    }
  }
  return -1;
}

/** The `(`…`)` that directly holds position `at`, escapes skipped. */
function enclosingGroup(sel: string, at: number): { open: number; close: number } | undefined {
  const open = openerBefore(sel, at);
  if (open < 0 || sel[open] !== "(") return undefined;
  const close = closerAfter(sel, open);
  return close < 0 ? undefined : { open, close };
}

/**
 * Whether the `.dark` at `index` is Tailwind's compiled dark variant on a utility: inside
 * `:is(.dark *)` or `:where(.dark, .dark *)` (light alike) that follows a class with an escaped
 * `:` (`.dark\:bg-card:is(.dark *)`, `.md\:dark\:x:is(.dark *)`). A hand-written
 * `.card:is(.dark *)` has no variant class, so it is still reported.
 */
function isVariantClass(sel: string, index: number, mode: string): boolean {
  const group = enclosingGroup(sel, index);
  if (!group) return false;
  const pseudo = /:(?:is|where)$/i.exec(sel.slice(0, group.open));
  if (!pseudo) return false;
  const inside = sel
    .slice(group.open + 1, group.close)
    .split(",")
    .map((part) => part.trim().split(/\s+/).join(" "));
  const under = `.${mode} *`;
  const shaped =
    inside.length === 1
      ? inside[0] === under
      : inside.length === 2 && inside[0] === `.${mode}` && inside[1] === under;
  const utility = sel.slice(compoundStart(sel, pseudo.index, true), pseudo.index);
  return shaped && /\\(?::|3a)/i.test(utility);
}

export const SOURCE_SELECTORS: SelectorReading = {
  list: (prelude) => selectorList(prelude, false),
  modeClasses: (selector) =>
    [...selector.matchAll(MODE_CLASS)].map((m) => ({ index: m.index ?? 0, mode: m[1] })),
  compoundAt: (selector, at) => selector.slice(compoundStart(selector, at, false)).trimStart(),
  isVariantClass: () => false
};

export const BUILT_SELECTORS: SelectorReading = {
  list: (prelude) => selectorList(prelude, true),
  modeClasses: (selector) =>
    [...selector.matchAll(MODE_CLASS_ESCAPED)]
      .map((m) => ({ index: m.index ?? 0, mode: m[1] }))
      .filter((m) => !isEscaped(selector, m.index)),
  compoundAt: (selector, at) => selector.slice(compoundStart(selector, at, true)).trimStart(),
  isVariantClass
};
