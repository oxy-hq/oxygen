// Test support: generated theme CSS as data. Not part of any entry. Good for the generated files
// only (no nested rules, no braces or semicolons inside values), which is all the tests read.

export interface CssBlock {
  selector: string;
  decls: [string, string][];
}

/** Every `selector { property: value; … }` rule, in order; comments and @import dropped. */
export function parseCssBlocks(css: string): CssBlock[] {
  const text = css.replace(/\/\*[\s\S]*?\*\//g, "").replace(/@import[^;]*;/g, "");
  const blocks: CssBlock[] = [];
  for (const m of text.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const decls: [string, string][] = [];
    for (const part of m[2].split(";")) {
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
    blocks.push({ selector: m[1].trim().replace(/\s+/g, " "), decls });
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
