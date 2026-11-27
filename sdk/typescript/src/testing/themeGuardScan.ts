// The structural scanner the theme guard reads CSS with (themeGuard.ts), and the reading of the
// SDK's own stylesheets it shares with `scripts/gen-theme-guard-rules.mjs`.
//
// Comments and the insides of strings and unquoted `url()`s are blanked (every character kept,
// so lines and offsets hold), then a walk over `{`, `;` and `}` reports each block's prelude and
// each declaration segment, so a selector such as `.btn--primary:focus-visible` is never read as
// a declaration of `--primary`.
//
// NO IMPORTS, and only syntax plain Node can strip: the generator script loads this file as it
// is, with no build.

const keepLines = (s: string) => s.replace(/[^\n]/g, " ");

/** Comments replaced by spaces, lines kept. Strings stay: an SDK prelude is matched on them. */
export function blankComments(css: string): string {
  return css.replace(/\/\*[\s\S]*?\*\//g, keepLines);
}

/** The insides of strings and unquoted url()s replaced by spaces, lines kept. */
export function blankStrings(text: string): string {
  return text
    .replace(/(["'])(?:\\.|(?!\1)[^\\\n])*\1/g, (m) => `${m[0]}${keepLines(m.slice(1, -1))}${m[0]}`)
    .replace(/url\(\s*([^"')\s][^)]*)\)/gi, (m, inner: string) =>
      m.replace(inner, keepLines(inner))
    );
}

/** What the walk reports, in source order. */
export interface WalkHandlers {
  /** A block opens: its prelude, trimmed, and the offset it starts at. */
  open(prelude: string, at: number): void;
  /** What sits between `{` (or `;`, or `}`) and the next `;` or `}`, and its offset. */
  segment(text: string, at: number): void;
  /** A block closes (after the segment before its `}`). */
  close(): void;
}

/** Every `{` opens a block, every `;` or `}` ends a segment, every `}` closes the innermost block. */
export function walk(text: string, on: WalkHandlers): void {
  let start = 0;
  for (let i = 0; i < text.length; i += 1) {
    const c = text[i];
    if (c === "{") {
      const raw = text.slice(start, i);
      const prelude = raw.trim();
      on.open(prelude, start + raw.indexOf(prelude));
      start = i + 1;
    } else if (c === "}" || c === ";") {
      on.segment(text.slice(start, i), start);
      if (c === "}") on.close();
      start = i + 1;
    }
  }
}

/**
 * A prelude as a minifier may have rewritten it: no whitespace, no quotes, lower case, and `::`
 * as `:` (`[data-oxy-mode="dark"]` is built as `[data-oxy-mode=dark]`, and lightningcss writes
 * `::after` as `:after`). Give it the prelude with its strings (comments blanked only).
 */
export function preludeKey(prelude: string): string {
  return prelude
    .replace(/[\s"']/g, "")
    .toLowerCase()
    .replace(/::/g, ":");
}

/** A property name as rules compare it: a custom property exactly, any other in lower case. */
export function propertyKey(name: string): string {
  return name.startsWith("--") ? name : name.toLowerCase();
}

/**
 * The properties each style rule of these stylesheets declares: `preludeKey` to its sorted
 * `propertyKey`s. One prelude can head several rules (`:where(:root)` at the top and inside
 * `@media`); they are one entry. At-rules are no entry, whatever they hold.
 */
export function collectSdkRules(stylesheets: Iterable<string>): Record<string, string[]> {
  const rules = new Map<string, Set<string>>();
  for (const css of stylesheets) {
    const comments = blankComments(css);
    const stack: (string | undefined)[] = [];
    walk(blankStrings(comments), {
      open(prelude, at) {
        const key = prelude.startsWith("@")
          ? undefined
          : preludeKey(comments.slice(at, at + prelude.length));
        if (key !== undefined && !rules.has(key)) rules.set(key, new Set());
        stack.push(key);
      },
      segment(text) {
        const key = stack[stack.length - 1];
        const colon = text.indexOf(":");
        if (key === undefined || colon < 0) return;
        const name = text.slice(0, colon).trim();
        if (name) rules.get(key)?.add(propertyKey(name));
      },
      close() {
        stack.pop();
      }
    });
  }
  return Object.fromEntries(
    [...rules.keys()].sort().map((key) => [key, [...(rules.get(key) ?? [])].sort()])
  );
}
