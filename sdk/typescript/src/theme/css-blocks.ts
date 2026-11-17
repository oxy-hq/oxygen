// Test support: generated theme CSS as data. Not part of any entry. Good for the generated files
// only (no nested rules, no braces or semicolons inside values), which is all the tests read.

export interface CssBlock {
  selector: string;
  decls: [string, string][];
}

function parseDecls(body: string): [string, string][] {
  const decls: [string, string][] = [];
  for (const part of body.split(";")) {
    const decl = part.trim();
    if (!decl) continue;
    const colon = decl.indexOf(":");
    decls.push([
      decl.slice(0, colon).trim(),
      decl
        .slice(colon + 1)
        .trim()
        .replace(/\s+/g, " ")
    ]);
  }
  return decls;
}

/**
 * Every `selector { property: value; … }` rule, in order; comments and @import dropped. A rule
 * inside a conditional at-rule is keyed with its prelude first (`@media (…) :where(:root)`), so it
 * never reads as the unconditional block of the same selector; `@layer` adds nothing to the key.
 */
export function parseCssBlocks(css: string): CssBlock[] {
  const text = css.replace(/\/\*[\s\S]*?\*\//g, "").replace(/@import[^;]*;/g, "");
  const blocks: CssBlock[] = [];
  const context: string[] = [];
  const pushed: boolean[] = [];
  let start = 0;
  for (let i = 0; i < text.length; i += 1) {
    if (text[i] === "}") {
      if (pushed.pop()) context.pop();
      start = i + 1;
    } else if (text[i] === "{") {
      const prelude = text.slice(start, i).trim().replace(/\s+/g, " ");
      const close = text.indexOf("}", i);
      const inner = text.indexOf("{", i + 1);
      if (inner !== -1 && inner < close) {
        const keyed = !prelude.startsWith("@layer");
        if (keyed) context.push(prelude);
        pushed.push(keyed);
        start = i + 1;
      } else {
        blocks.push({
          selector: [...context, prelude].join(" "),
          decls: parseDecls(text.slice(i + 1, close))
        });
        i = close;
        start = close + 1;
      }
    }
  }
  return blocks;
}

/** selector -> property -> value; throws on a repeated selector or property. */
export function cssBlockMap(css: string): Map<string, Map<string, string>> {
  const out = new Map<string, Map<string, string>>();
  for (const block of parseCssBlocks(css)) {
    if (out.has(block.selector)) throw new Error(`selector ${block.selector} appears twice`);
    const decls = new Map<string, string>();
    for (const [prop, value] of block.decls) {
      if (decls.has(prop)) throw new Error(`${block.selector}: ${prop} appears twice`);
      decls.set(prop, value);
    }
    out.set(block.selector, decls);
  }
  return out;
}

/** The alias block's selector in theme.css (build-theme.mjs ALIAS_SELECTOR). */
export const ALIAS_SELECTOR = ":where(html)";

/**
 * A direction's two selectors in theme.css, as build-theme.mjs writes them: the shipped theme (the
 * first in THEMES) on `:where(:root)`, any other under data-oxy-theme; dark is the `.dark` class or
 * data-oxy-mode="dark".
 */
export function themeSelectors(id: string, shippedId: string): { light: string; dark: string } {
  const root = id === shippedId ? ":root" : `:root[data-oxy-theme="${id}"]`;
  return {
    light: `:where(${root})`,
    dark: `:where(${root}.dark, ${root}[data-oxy-mode="dark"])`
  };
}
