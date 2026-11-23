import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

/**
 * The `design-reference` scope (the shell and the home) is the SDK theme written on a class
 * instead of `:root`. Its values are copied by hand, so this fails the moment
 * one stops matching the web app's copy of the SDK theme (`sdk-theme.css`, which
 * the SDK's generator keeps byte-identical to the shipped theme).
 */
const here = path.dirname(fileURLToPath(import.meta.url));
const read = (file: string) => readFileSync(path.join(here, file), "utf8");
const strip = (css: string) => css.replace(/\/\*[\s\S]*?\*\//g, "");

/** The declarations of the first rule whose selector is exactly `selector`. */
function block(css: string, selector: string): Map<string, string> {
  const start = css.indexOf(`${selector} {`);
  if (start < 0) throw new Error(`no rule for ${selector}`);
  const open = css.indexOf("{", start);
  const body = css.slice(open + 1, css.indexOf("\n}", open));
  const out = new Map<string, string>();
  for (const [, name, value] of body.matchAll(/(--[a-z0-9-]+):\s*([^;]+);/g)) {
    out.set(name, value.replace(/\s+/g, " ").trim());
  }
  return out;
}

const sdk = strip(read("../shadcn/sdk-theme.css"));
const scope = strip(read("theme.css"));
const modes = [
  { mode: "light", sdk: block(sdk, ":where(:root)"), scope: block(scope, ".design-reference") },
  {
    mode: "dark",
    sdk: block(sdk, ':where(:root.dark, :root[data-oxy-mode="dark"])'),
    scope: block(scope, ".dark .design-reference,\n.dark.design-reference")
  }
];

/**
 * Names the scope sets to something other than the SDK's value, or that the SDK
 * does not have, and why. Anything else must equal the SDK's value.
 */
const ON_PURPOSE: Record<string, string> = {
  "--border": "shadcn's `border` utility draws every card: the hairline, not the 3:1 edge",
  "--input": "shadcn's input edge: the SDK's field edge",
  "--ring": "shadcn's focus ring: the SDK's focus ring colour",
  "--sidebar-background": "the web app's rail and top bar read this name; the SDK has `--sidebar`",
  "--control-border": "a quiet control's edge, which the SDK has no name for",
  "--radius": "shadcn's radius: square, as every SDK radius is"
};

describe("the design-reference scope", () => {
  for (const { mode, sdk: theirs, scope: ours } of modes) {
    it(`matches the SDK theme in ${mode} mode`, () => {
      expect(ours.size).toBeGreaterThan(30);
      for (const [name, value] of ours) {
        if (name in ON_PURPOSE) continue;
        expect(theirs.has(name), `${name} (${mode}) is not an SDK theme token`).toBe(true);
        expect(value, `${name} (${mode}) differs from sdk-theme.css`).toBe(theirs.get(name));
      }
    });
  }

  it("keeps its corners square, as the SDK theme's are", () => {
    expect(modes[0]?.sdk.get("--radius-surface")).toBe("0px");
    expect(scope).toMatch(/\.design-reference \*:not\(\.rounded-full\) \{\s*border-radius: 0;/);
  });
});
