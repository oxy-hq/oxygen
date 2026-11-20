// Shared by the charts' tests (not itself a test file): what a drawn chart paints with. Every
// stroke, fill and stop colour must be a theme token, the chart's own ground property, a gradient
// of its own, `none` or `transparent`; a raw colour or an unknown name is reported.

import contract from "../../theme/contract/contract.json";

const TOKENS = new Set((contract as { tokens: { name: string }[] }).tokens.map((t) => t.name));
const PAINT = ["stroke", "fill", "stop-color"];

export function paints(root: ParentNode): { tokens: Set<string>; off: string[] } {
  const tokens = new Set<string>();
  const off: string[] = [];
  for (const el of root.querySelectorAll("*"))
    for (const attr of PAINT) {
      const value = el.getAttribute(attr);
      if (value === null || value === "none" || value === "transparent") continue;
      if (/^url\(#[^)]+\)$/.test(value)) continue;
      const name = /^var\((--[a-z0-9-]+)\)$/.exec(value)?.[1];
      if (name && (TOKENS.has(name) || name === "--oxy-ui-chart-ground")) tokens.add(name);
      else off.push(`<${el.tagName.toLowerCase()} ${attr}="${value}">`);
    }
  return { tokens, off };
}
