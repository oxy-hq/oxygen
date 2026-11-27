/**
 * `css-digest.json`: which declarations of a build's CSS it lists, and that
 * it stays inside its size. The reader is this package's own (`css-digest.ts`
 * says why); the SDK theme's names are the guard's real ones.
 */

import { describe, expect, it } from "vitest";
import { cssDigest, cssDigestFile, MAX_DIGEST_BYTES } from "./css-digest.js";

function digest(css: string, file = "assets/index-abc.css") {
  return cssDigest([{ file, css }]);
}

describe("cssDigest", () => {
  it("lists the declarations that set a literal colour or a font size, by rule", () => {
    const found = digest(`
      .hero { color: #1a1a1a; padding: 4px; font-size: 11px; background: var(--card) }
      .note { border: 1px solid rgb(0 0 0 / 0.2); margin: 0 }
      .plain { display: flex }
    `);
    expect(found).toEqual({
      rules: [
        {
          file: "assets/index-abc.css",
          selector: ".hero",
          declarations: ["color: #1a1a1a", "font-size: 11px"]
        },
        {
          file: "assets/index-abc.css",
          selector: ".note",
          declarations: ["border: 1px solid rgb(0 0 0 / 0.2)"]
        }
      ],
      omitted: 0
    });
  });

  it("reads minified CSS, and says which at-rules a rule sits in", () => {
    const found = digest(
      "@media (min-width:40rem){.title{font-size:var(--text-lg);color:white}}.x{font:12px/1.4 Inter}.y{font:inherit}"
    );
    expect(found.rules).toEqual([
      {
        file: "assets/index-abc.css",
        selector: ".title",
        within: ["@media (min-width:40rem)"],
        // A font size is listed whatever sets it; a colour only when it is a literal.
        declarations: ["font-size: var(--text-lg)", "color: white"]
      },
      { file: "assets/index-abc.css", selector: ".x", declarations: ["font: 12px/1.4 Inter"] }
    ]);
  });

  it("leaves out the SDK theme's own names and Tailwind's palette and plumbing", () => {
    const found = digest(`
      @layer theme { :root, :host { --color-red-500: oklch(63.7% 0.237 25.331) } }
      :root, :host { --color-brand: #ff0000 }
      @layer properties { *, ::before { --tw-shadow-color: #0000 } }
      @property --tw-ring-color { syntax: "*"; inherits: false; initial-value: #000 }
      :where(:root) { --card: oklch(1 0 0); --foreground: #111 }
      .a { --tw-shadow: 0 0 #0000; box-shadow: var(--tw-shadow) }
      @font-face { font-family: Inter; src: url(/x.woff2) }
      .own { --acme-accent: #0af }
    `);
    // What is left is the app's own custom colour.
    expect(found.rules).toEqual([
      { file: "assets/index-abc.css", selector: ".own", declarations: ["--acme-accent: #0af"] }
    ]);
  });

  it("leaves out the SDK's own rules, by their class and attribute names", () => {
    const found = digest(`
      .oxy-ui-btn { font-size: 13px; color: #fff }
      :root:not([data-oxy-touch-targets=off]) .oxy-rail__item { font-size: 16px }
      [data-oxy-mode="dark"] .card { color: black }
      .app-btn { font-size: 13px }
    `);
    expect(found.rules.map((rule) => rule.selector)).toEqual([".app-btn"]);
  });

  it("is not fooled by a comment, a string or a url", () => {
    const found = digest(`
      /* .ghost { color: #fff } */
      .icon { background: url(data:image/svg+xml;utf8,<svg fill="#000"/>) }
      .quote::before { content: "black; color: #fff }" }
      .real { color: #abc }
    `);
    expect(found.rules).toEqual([
      { file: "assets/index-abc.css", selector: ".real", declarations: ["color: #abc"] }
    ]);
  });

  it("reads the <style> blocks of an HTML file, and nothing else of it", () => {
    const found = digest(
      '<html><head><style>.boot{color:#123456}</style></head><body style="color:#fff">#000</body></html>',
      "index.html"
    );
    expect(found.rules).toEqual([
      { file: "index.html", selector: ".boot", declarations: ["color: #123456"] }
    ]);
  });

  it("stops where it would pass its size, and counts what it left out", () => {
    const css = Array.from(
      { length: 4000 },
      (_, i) => `.rule-${i}{color:#${String(i).padStart(6, "0")}}`
    );
    const found = cssDigest([{ file: "assets/big.css", css: css.join("") }]);
    const bytes = cssDigestFile([{ file: "assets/big.css", css: css.join("") }]).byteLength;
    expect(bytes).toBeLessThanOrEqual(MAX_DIGEST_BYTES);
    // Close to full, not cut far short of it.
    expect(bytes).toBeGreaterThan(MAX_DIGEST_BYTES - 200);
    expect(found.rules.length).toBeGreaterThan(100);
    expect(found.rules.length + found.omitted).toBe(4000);
    expect(found.rules[0]?.selector).toBe(".rule-0");
  });

  it("is an empty digest for a build with no such declaration", () => {
    expect(cssDigest([])).toEqual({ rules: [], omitted: 0 });
    expect(JSON.parse(Buffer.from(cssDigestFile([])).toString("utf8"))).toEqual({
      rules: [],
      omitted: 0
    });
  });
});
