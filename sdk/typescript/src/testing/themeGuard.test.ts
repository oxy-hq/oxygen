// @vitest-environment node
//
// checkThemeCss: one planted failure per rule, the cases each rule must let through (the ones the
// five app guards disagreed on, INVENTORY section 6), HTML <style> blocks, and file:line.

import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, describe, expect, it } from "vitest";
import { checkThemeCss, formatThemeFindings, SDK_THEME_NAMES } from "./themeGuard";

const here = dirname(fileURLToPath(import.meta.url));
const scratch = mkdtempSync(join(tmpdir(), "oxy-theme-guard-"));
afterAll(() => rmSync(scratch, { recursive: true, force: true }));

const check = (css: string, file = "app.css") => checkThemeCss([{ file, css }]);
const rules = (css: string, file?: string) =>
  check(css, file).map((f) => `${f.line} ${f.rule} ${f.subject}`);

describe("checkThemeCss", () => {
  it("knows every theme.css name, the 2.30 ones included", () => {
    expect(SDK_THEME_NAMES.size).toBeGreaterThan(280);
    for (const n of [
      "--segment-selected",
      "--card",
      "--type-scale-sm-size",
      "--radius-md",
      "--oxy-duration-fast"
    ])
      expect(SDK_THEME_NAMES.has(n), n).toBe(true);
  });

  it("restates-sdk-token: a theme name declared outside @theme, at its line", () => {
    const css = [".panel { color: var(--foreground); }", ":root {", "  --card: #ffffff;", "}"].join(
      "\n"
    );
    expect(rules(css)).toEqual(["3 restates-sdk-token --card"]);
  });

  it("lets an app's own names through, and the aliases inside @theme", () => {
    expect(
      check(
        [
          ":root { --wh-ground: #101418; --app-hue: 210; }",
          "@theme inline { --color-card: var(--card); --radius-md: var(--radius-control); }",
          "@theme { --radius: 0; --font-sans: system-ui, sans-serif; }"
        ].join("\n")
      )
    ).toEqual([]);
  });

  it("literal-colour-in-theme: a hex, an rgb() or white pinned in @theme", () => {
    const css = [
      "@theme {",
      "  --color-background: #0a0a0a;",
      "  --color-scrim: rgb(0 0 0 / 0.5);",
      "  --color-paper: white;",
      "  --radius: 0;",
      "}"
    ].join("\n");
    expect(rules(css)).toEqual([
      "2 literal-colour-in-theme --color-background",
      "3 literal-colour-in-theme --color-scrim",
      "4 literal-colour-in-theme --color-paper"
    ]);
  });

  it("bare-mode-class: .dark or .light not on :root or html", () => {
    const css = [".dark .card { color: inherit; }", ".card:is(.light *) { color: inherit; }"].join(
      "\n"
    );
    expect(rules(css)).toEqual([
      "1 bare-mode-class .dark .card",
      "2 bare-mode-class .card:is(.light *)"
    ]);
  });

  it("lets :root:not(.dark), :root.dark, html.light and Tailwind's dark variant through", () => {
    expect(
      check(
        [
          "@custom-variant dark (&:is(.dark *));",
          "@custom-variant light { &:is(.light *) { @slot; } }",
          ":root:not(.dark) .world-model-vignette { opacity: 0.6; }",
          ":root.dark .map, html.light .map { filter: none; }",
          ".dark-mode-hint, .lightbox { display: block; }"
        ].join("\n")
      )
    ).toEqual([]);
  });

  it("own-color-scheme: the theme sets color-scheme, so an app never does", () => {
    expect(rules(".app {\n  color-scheme: dark;\n}")).toEqual(["2 own-color-scheme color-scheme"]);
  });

  it("own-mode-block: [data-theme], [data-oxy-mode] and prefers-color-scheme", () => {
    const css = [
      '[data-theme="dark"] .panel { opacity: 1; }',
      ':root[data-oxy-mode="dark"] .panel { opacity: 1; }',
      "@media (prefers-color-scheme: dark) {",
      "  .panel { opacity: 1; }",
      "}"
    ].join("\n");
    expect(check(css).map((f) => `${f.line} ${f.rule}`)).toEqual([
      "1 own-mode-block",
      "2 own-mode-block",
      "3 own-mode-block"
    ]);
  });

  it("never reads a selector as a declaration: .btn--primary:focus-visible is no --primary", () => {
    const html = [
      "<!doctype html>",
      "<html><head><style>",
      ".btn--primary:focus-visible { outline: 2px solid var(--ring); }",
      ".callout--error { border-color: var(--status-error); }",
      "</style></head></html>"
    ].join("\n");
    expect(check(html, "index.html")).toEqual([]);
    expect(SDK_THEME_NAMES.has("--primary")).toBe(true);
  });

  it("reads index.html <style> blocks at their own lines (Bookkeeping's plant)", () => {
    const html = [
      "<!doctype html>",
      "<html>",
      "  <head>",
      "    <style>",
      "      :root.dark {",
      "        --select-chevron: none;",
      "        color-scheme: dark;",
      "      }",
      "    </style>",
      "  </head>",
      "</html>"
    ].join("\n");
    expect(rules(html, "index.html")).toEqual([
      "6 restates-sdk-token --select-chevron",
      "7 own-color-scheme color-scheme"
    ]);
  });

  it("ignores comments and is not thrown by braces inside strings", () => {
    const css = [
      "/* :root { --card: red; } .dark .x {} */",
      '.x::before { content: "{ --card: red; }"; }',
      ":root { --muted: #eee; }"
    ].join("\n");
    expect(rules(css)).toEqual(["3 restates-sdk-token --muted"]);
  });

  it("reads files from disk and reports file:line", () => {
    const file = join(scratch, "index.css");
    writeFileSync(
      file,
      "@theme inline {\n  --color-card: var(--card);\n}\n\n.dark .x {\n  --border: #000;\n}\n"
    );
    const found = checkThemeCss(["index.css"], { cwd: scratch });
    expect(found.map((f) => `${f.at} ${f.rule}`)).toEqual([
      "index.css:5 bare-mode-class",
      "index.css:6 restates-sdk-token"
    ]);
    expect(formatThemeFindings(found)).toMatch(/^index\.css:5 bare-mode-class: /);
  });

  it("passes the SDK's own tailwind.css, which an app imports", () => {
    expect(checkThemeCss([join(here, "..", "ui", "tailwind.css")])).toEqual([]);
  });
});
