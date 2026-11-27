// @vitest-environment node
//
// checkThemeCss(sources, { built: true }): the guard on an app's BUILT CSS (Vite + Tailwind v4
// output, minified, with the SDK's stylesheets bundled in), which is what the platform's
// design-system check reads. Ported case for case from the server's Rust copy of the guard, which
// this replaced: until PR #4250 it lived in
// crates/api-custom-apps/src/custom_apps_build_checks/design_system/, and each test here names
// the Rust case it came from (theme_guard_tests.rs, theme_guard_compiled_tests.rs,
// design_system_tests.rs), so the two can be compared in the history.
//
//   - every source case of themeGuard.test.ts reads the same with `built`
//   - the built readings: Tailwind's `@layer theme` and its unlayered `:root, :host` rule, the
//     SDK's own rules, escaped classes
//   - `built` unset reports on built CSS what it always did
//   - themeGuardSdkRules.gen.json is what the SDK's stylesheets declare
//   - `@oxy-hq/sdk/theme-guard` imports the guard and nothing else

import { existsSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { transform } from "esbuild";
import { describe, expect, it } from "vitest";
import { buildUiCss } from "../../scripts/build-ui-css.mjs";
import { generateSdkRules, sdkStylesheets } from "../../scripts/gen-theme-guard-rules.mjs";
import * as entry from "../theme-guard";
import * as testingEntry from "./index";
import { checkThemeCss, formatThemeFindings, SDK_THEME_NAMES } from "./themeGuard";
import committedSdkRules from "./themeGuardSdkRules.gen.json";

const here = dirname(fileURLToPath(import.meta.url));
const pkg = resolve(here, "..", "..");
const sdkFile = (rel: string) => readFileSync(join(pkg, rel), "utf8");

const check = (css: string, file = "app.css") => checkThemeCss([{ file, css }], { built: true });
const rules = (css: string, file?: string) =>
  check(css, file).map((f) => `${f.line} ${f.rule} ${f.subject}`);
/** The same, with `built` unset: what the guard reported before it could read built CSS. */
const sourceRules = (css: string, file = "app.css") =>
  checkThemeCss([{ file, css }]).map((f) => `${f.line} ${f.rule} ${f.subject}`);

/**
 * The SDK's own blocks as an app's bundle carries them: theme.css's light, dark,
 * reduced-transparency and derived blocks, motion.css's `:root`, and shell.css's dark scope,
 * minified onto one line. The shapes are the ones `@tailwindcss/node` 4.3 and lightningcss write
 * (`:where(:root.dark,:root[data-oxy-mode=dark])`, `:after`, `.dark\:bg-card:is(.dark *)`).
 */
const SDK_BLOCKS_MINIFIED = [
  ":where(:root){color-scheme:light;--oxy-theme-contract:1;--scrim-blur:8px;--radius-control:0px;",
  '--font-sans:-apple-system,BlinkMacSystemFont,"Inter",system-ui,sans-serif;--card:#fff;',
  `--select-chevron:url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg'%3E%3C/svg%3E")}`,
  ":where(:root.dark,:root[data-oxy-mode=dark]){color-scheme:dark;--background:#0a0a0a;--card:#1a1a1a}",
  "@media (prefers-reduced-transparency:reduce){:where(:root){--scrim-blur:0px}}",
  "@layer theme{:where(html){--input:var(--field-border);--radius-sm:var(--radius-control)}}",
  ":root{--oxy-duration-fast:.12s;--oxy-ease-out:cubic-bezier(.16,1,.3,1)}",
  ".dark .oxy-shell-scope,.oxy-shell-scope.dark{--oxy-shell-background:var(--background,#0a0a0a)}"
].join("");

/** Tailwind's compiled `@theme`: its own names, and its default palette as literals. */
const TAILWIND_THEME_LAYER = [
  "@layer theme{:root,:host{--font-sans:ui-sans-serif,system-ui,sans-serif;",
  "--radius-sm:.25rem;--color-card:var(--card);",
  "--color-red-500:oklch(63.7% .237 25.331);--color-white:#fff}}"
].join("");

/** Tailwind's compiled dark variant, on every shape of utility it writes it for. */
const DARK_VARIANT_UTILITIES = [
  String.raw`.dark\:text-white:is(.dark *){color:var(--color-white)}`,
  String.raw`.dark\:bg-card:is(.dark *){background-color:var(--card)}`,
  String.raw`.dark\:bg-card:where(.dark,.dark *){background-color:var(--card)}`,
  String.raw`.hover\:dark\:bg-muted:hover:is(.dark *){background-color:var(--muted)}`,
  String.raw`@media (width>=48rem){@media (hover:hover){.md\:dark\:hover\:bg-muted:is(.dark *):hover{background-color:var(--muted)}}}`,
  String.raw`.group-hover\:dark\:text-white:is(:where(.group):hover *):is(.dark *){color:#fff}`,
  String.raw`.\32 xl\:dark\:bg-card:is(.dark *){background-color:var(--card)}`,
  String.raw`.light\:bg-card:is(.light *){background-color:var(--card)}`
].join("\n");

const ESCAPED_DARK_CLASSES = String.raw`.dark\:bg-card{background:var(--card)}.w-1\.dark{width:1px}`;

/**
 * An app's own lines, on the import order `@oxy-hq/sdk/tailwind.css` asks for (it first, then
 * `tailwindcss`, then the SDK's stylesheets). The source reading allows every one of them.
 */
const APP_THEME_SOURCE = [
  "@custom-variant dark (&:is(.dark *));",
  "@theme { --color-card: var(--card); --color-brand: var(--primary); }",
  ".app-panel { background: var(--card); }"
].join("\n");

/**
 * What `@tailwindcss/node` 4.3.3 and lightningcss wrote for APP_THEME_SOURCE with the classes
 * `bg-card bg-brand p-4 text-red-500 dark:bg-card`: the banner, then, from the one line that
 * follows it, the utilities layer, Tailwind's compiled `@theme` and the app's rule, each as
 * written and in that order. The theme, base, oxy-ui and components layers and the SDK's
 * stylesheets around them are cut. The first `@theme` Tailwind met (the SDK's tailwind.css) is
 * outside every layer, so the compiled `@theme` is an unlayered `:root,:host`, not `@layer theme`.
 */
const APP_THEME_COMPILED = [
  "/*! tailwindcss v4.3.3 | MIT License | https://tailwindcss.com */",
  [
    String.raw`@layer utilities{.bg-brand{background-color:var(--color-brand)}.bg-card{background-color:var(--color-card)}.p-4{padding:calc(var(--spacing) * 4)}.text-red-500{color:var(--color-red-500)}.dark\:bg-card:is(.dark *){background-color:var(--color-card)}}`,
    ":root,:host{--color-card:var(--card);--color-brand:var(--primary);--color-red-500:oklch(63.7% .237 25.331);--spacing:.25rem;--default-font-family:var(--font-sans);--default-mono-font-family:var(--font-mono)}",
    ".app-panel{background:var(--card)}"
  ].join("")
].join("\n");

/** The SDK's stylesheets as a bundle carries them: theme, motion, shell and the built ui.css. */
const sdkBundle = () =>
  [
    sdkFile("src/theme/theme.css"),
    sdkFile("src/motion/motion.css"),
    sdkFile("src/shell/shell.css"),
    buildUiCss(pkg)
  ].join("\n");
const minify = async (css: string) => (await transform(css, { loader: "css", minify: true })).code;

// theme_guard_tests.rs: every case of themeGuard.test.ts, with `built`.
describe("checkThemeCss, built: the source cases read the same", () => {
  // theme_guard_knows_every_theme_css_name
  it("knows every theme.css name", () => {
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

  // theme_guard_restates_a_theme_name_outside_theme_at_its_line
  it("restates-sdk-token: a theme name declared outside @theme, at its line", () => {
    const css = [".panel { color: var(--foreground); }", ":root {", "  --card: #ffffff;", "}"].join(
      "\n"
    );
    expect(rules(css)).toEqual(["3 restates-sdk-token --card"]);
  });

  // theme_guard_lets_app_names_and_theme_aliases_through
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

  // theme_guard_literal_colour_in_theme
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

  // theme_guard_bare_mode_class
  it("bare-mode-class: .dark or .light not on :root or html", () => {
    const css = [".dark .card { color: inherit; }", ".card:is(.light *) { color: inherit; }"].join(
      "\n"
    );
    expect(rules(css)).toEqual([
      "1 bare-mode-class .dark .card",
      "2 bare-mode-class .card:is(.light *)"
    ]);
  });

  // theme_guard_lets_root_keyed_modes_and_the_custom_variant_through
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

  // theme_guard_own_color_scheme
  it("own-color-scheme: the theme sets color-scheme, so an app never does", () => {
    expect(rules(".app {\n  color-scheme: dark;\n}")).toEqual(["2 own-color-scheme color-scheme"]);
  });

  // theme_guard_own_mode_block
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

  // theme_guard_never_reads_a_selector_as_a_declaration
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

  // theme_guard_reads_html_style_blocks_at_their_own_lines
  it("reads index.html <style> blocks at their own lines", () => {
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

  // theme_guard_ignores_comments_and_braces_inside_strings
  it("ignores comments and is not thrown by braces inside strings", () => {
    const css = [
      "/* :root { --card: red; } .dark .x {} */",
      '.x::before { content: "{ --card: red; }"; }',
      ":root { --muted: #eee; }"
    ].join("\n");
    expect(rules(css)).toEqual(["3 restates-sdk-token --muted"]);
  });

  // theme_guard_reports_file_line_and_formats_it (the bundle is held in memory, so as text)
  it("reports file:line and formats it", () => {
    const css =
      "@theme inline {\n  --color-card: var(--card);\n}\n\n.dark .x {\n  --border: #000;\n}\n";
    const found = check(css, "index.css");
    expect(found.map((f) => `${f.at} ${f.rule}`)).toEqual([
      "index.css:5 bare-mode-class",
      "index.css:6 restates-sdk-token"
    ]);
    expect(formatThemeFindings(found)).toMatch(/^index\.css:5 bare-mode-class: /);
  });

  // theme_guard_passes_the_sdks_own_tailwind_css
  it("passes the SDK's own tailwind.css, which an app imports", () => {
    expect(check(sdkFile("src/ui/tailwind.css"), "tailwind.css")).toEqual([]);
  });

  // theme_guard_finding_serialises_with_the_kebab_rule_id
  it("a finding serialises as the check stores it, with the kebab rule id", () => {
    const found = check(":root {\n  --card: #fff;\n}", "src/index.css");
    expect(JSON.parse(JSON.stringify(found))).toEqual([
      {
        file: "src/index.css",
        line: 2,
        at: "src/index.css:2",
        rule: "restates-sdk-token",
        subject: "--card",
        message: "restates --card, which theme.css defines; remove it"
      }
    ]);
  });

  // theme_guard_sorts_by_file_then_line
  it("sorts by file, then line", () => {
    const found = checkThemeCss(
      [
        { file: "b.css", css: "\n\n.dark .x {}\n:root { --card: red; }" },
        { file: "a.css", css: "\n:root { color-scheme: dark; }" }
      ],
      { built: true }
    );
    expect(found.map((f) => f.at)).toEqual(["a.css:2", "b.css:3", "b.css:4"]);
  });
});

// theme_guard_compiled_tests.rs
describe("checkThemeCss, built: `@layer theme` reads as `@theme`", () => {
  // theme_guard_compiled_tailwind_theme_layer_reports_nothing
  it("Tailwind's compiled theme layer reports nothing", () => {
    for (const name of ["--font-sans", "--radius-sm", "--color-card"])
      expect(SDK_THEME_NAMES.has(name), `${name} is an SDK name`).toBe(true);
    expect(check(TAILWIND_THEME_LAYER, "assets/index.css")).toEqual([]);
  });

  // theme_guard_compiled_only_a_real_theme_block_reports_a_literal_colour
  it("only a real @theme block reports a literal colour", () => {
    const found = check("@theme{--color-red-500:oklch(63.7% .237 25.331)}");
    expect(found.map((f) => `${f.line} ${f.rule} ${f.subject}`)).toEqual([
      "1 literal-colour-in-theme --color-red-500"
    ]);
    expect(found[0].message).toBe(
      "--color-red-500 pins the literal colour oklch( in @theme; point it at a theme token (var(--…))"
    );
  });

  // theme_guard_compiled_other_layers_still_restate
  it("other layers still restate", () => {
    const css = "@layer base{:root{--card:#fff}}@layer theme-x{:root{--muted:#eee}}";
    expect(rules(css)).toEqual(["1 restates-sdk-token --card", "1 restates-sdk-token --muted"]);
  });

  // Not a Rust case, but a rule the Rust guard stated: a hand-written `.dark .x` in that layer
  // gets no pass, and only its custom properties are read as inside `@theme`.
  it("still reads the selectors inside the theme layer", () => {
    expect(rules("@layer theme{.dark .x{--card:#fff}}")).toEqual(["1 bare-mode-class .dark .x"]);
  });
});

// Not Rust cases: a reading the Rust guard lacked. An app's own `@theme`, which the source
// reading allows, is not reported once Tailwind has compiled it.
describe("checkThemeCss, built: Tailwind's unlayered `:root, :host` reads as `@theme`", () => {
  it("an app's own @theme, compiled outside every layer, reports nothing", () => {
    for (const name of ["--color-card", "--color-brand"])
      expect(SDK_THEME_NAMES.has(name), `${name} is an SDK name`).toBe(true);
    expect(checkThemeCss([{ file: "src/index.css", css: APP_THEME_SOURCE }])).toEqual([]);
    expect(check(APP_THEME_COMPILED, "assets/index.css")).toEqual([]);
  });

  it("`:root` alone beside it still restates", () => {
    expect(rules(`${APP_THEME_COMPILED}\n:root{--card:#fff}`, "assets/index.css")).toEqual([
      "3 restates-sdk-token --card"
    ]);
  });

  it("`:root, :host` with anything added, or the other way round, still restates", () => {
    expect(rules(":root,:host,.x{--card:#fff}")).toEqual(["1 restates-sdk-token --card"]);
    expect(rules(":host,:root{--card:#fff}")).toEqual(["1 restates-sdk-token --card"]);
  });

  it("the same rule inside another layer still restates", () => {
    expect(rules("@layer base{:root,:host{--card:#fff}}")).toEqual(["1 restates-sdk-token --card"]);
  });

  it("only the top-level rule gets the pass: inside an at-rule it still restates", () => {
    expect(rules("@media (min-width:1px){:root,:host{--card:#fff}}")).toEqual([
      "1 restates-sdk-token --card"
    ]);
    expect(rules("@supports (display:grid){:root,:host{--card:#fff}}")).toEqual([
      "1 restates-sdk-token --card"
    ]);
  });

  it("is matched however it is spaced, and only its custom properties get the pass", () => {
    expect(check(":root, :host {\n  --card: #fff;\n}")).toEqual([]);
    expect(rules(":root,:host{color-scheme:dark;--card:#fff}")).toEqual([
      "1 own-color-scheme color-scheme"
    ]);
    expect(rules(":root,:host{.dark .x{--card:#fff}}")).toEqual([
      "1 bare-mode-class .dark .x",
      "1 restates-sdk-token --card"
    ]);
  });

  it("`built` unset: `:root, :host` reports exactly as it did", () => {
    expect(sourceRules(":root,:host{--card:#fff}")).toEqual(["1 restates-sdk-token --card"]);
    expect(sourceRules(APP_THEME_COMPILED)).toEqual([
      String.raw`2 bare-mode-class .dark\:bg-card:is(.dark *)`,
      String.raw`2 bare-mode-class .dark\:bg-card:is(.dark *)`,
      "2 restates-sdk-token --color-card",
      "2 restates-sdk-token --color-brand"
    ]);
  });
});

describe("checkThemeCss, built: the SDK's own rules are skipped", () => {
  // theme_guard_compiled_sdk_blocks_report_nothing
  it("the SDK's blocks, minified, report nothing", () => {
    expect(check(SDK_BLOCKS_MINIFIED, "assets/index.css")).toEqual([]);
  });

  // theme_guard_compiled_every_sdk_stylesheet_passes_on_its_own: a rule the SDK adds that the
  // guard would report (an at-rule, say) fails here, not on every app.
  it("every SDK stylesheet passes on its own", () => {
    for (const rel of sdkStylesheets()) {
      const found = check(sdkFile(rel), rel);
      expect(formatThemeFindings(found), rel).toBe("");
    }
  });

  // The SDK's whole CSS as a bundle carries it, and the same minified (esbuild, as Vite's
  // default CSS minifier): unquoted attribute values, no spaces, merged rules.
  it("the SDK's bundled CSS reports nothing, minified or not", async () => {
    const bundle = sdkBundle();
    const minified = await minify(bundle);
    expect(minified.length).toBeLessThan(bundle.length * 0.8);
    expect(formatThemeFindings(check(bundle, "assets/index.css"))).toBe("");
    expect(formatThemeFindings(check(minified, "assets/index.css"))).toBe("");
  });

  it("an app's own rules beside the SDK's bundled CSS are still reported", async () => {
    const app = ".dark .card{color:red}\n:root{--card:#fff}\nhtml{color-scheme:light}";
    const minified = await minify(sdkBundle());
    expect(
      rules(`${minified}\n${app}`, "assets/index.css").map((r) => r.replace(/^\d+ /, ""))
    ).toEqual([
      "bare-mode-class .dark .card",
      "restates-sdk-token --card",
      "own-color-scheme color-scheme"
    ]);
  });

  // theme_guard_compiled_app_root_after_the_sdk_still_restates
  it("an app's :root after the SDK's still restates", () => {
    const css = `${SDK_BLOCKS_MINIFIED}\n.app-panel{background:var(--card)}\n:root{--card:#fff}\n`;
    expect(rules(css, "assets/index.css")).toEqual(["3 restates-sdk-token --card"]);
  });

  // theme_guard_compiled_app_declaration_merged_into_an_sdk_rule_still_restates: lightningcss
  // merges neighbours with one selector, and an app's `:root` folded into motion.css's still
  // reports what the SDK does not declare there.
  it("an app declaration merged into an SDK rule still restates", () => {
    const css = ":root{--oxy-duration-fast:.12s;--card:#fff;--app-hue:210}";
    expect(rules(css, "assets/index.css")).toEqual(["1 restates-sdk-token --card"]);
  });

  // theme_guard_compiled_app_color_scheme_reports
  it("an app's own color-scheme reports", () => {
    const css = `${SDK_BLOCKS_MINIFIED}\nhtml{color-scheme:light}`;
    expect(rules(css, "assets/index.css")).toEqual(["2 own-color-scheme color-scheme"]);
  });
});

describe("checkThemeCss, built: selectors are read with their escapes", () => {
  // theme_guard_compiled_dark_variant_utilities_report_nothing
  it("Tailwind's dark variant utilities report nothing", () => {
    expect(check(DARK_VARIANT_UTILITIES, "assets/index.css")).toEqual([]);
  });

  // theme_guard_compiled_an_escaped_dark_class_is_not_dark
  it("an escaped dark class is not .dark", () => {
    expect(check(ESCAPED_DARK_CLASSES, "assets/index.css")).toEqual([]);
  });

  // Not a Rust case: the hex form of an escape. The space that ends `\3a ` or `\32 ` is part of
  // the class, and a space after any other class is still a combinator.
  it("reads a hex escape, and the space that ends it, as part of the class", () => {
    expect(check(String.raw`.dark\3a bg-card:is(.dark *){background:var(--card)}`)).toEqual([]);
    expect(rules(String.raw`.\32 xl .dark .card{color:red}`)).toEqual([
      String.raw`1 bare-mode-class .\32 xl .dark .card`
    ]);
  });

  // theme_guard_compiled_bare_dark_ancestor_still_reports
  it("a bare .dark ancestor still reports, and so does a hand-written :is(.dark *)", () => {
    const css = `${SDK_BLOCKS_MINIFIED}\n${String.raw`.dark\:bg-card:is(.dark *){background:var(--card)}.dark .card{color:red}.card:is(.dark *){color:red}`}`;
    expect(rules(css, "assets/index.css")).toEqual([
      "2 bare-mode-class .dark .card",
      "2 bare-mode-class .card:is(.dark *)"
    ]);
  });
});

// design_system_tests.rs: what the check's verdict reads from the guard. The verdict itself (the
// SDK floor, the summary, the status) is the runner's, not the guard's.
describe("checkThemeCss, built: what the design-system check reads", () => {
  const findings = (css: string) => check(css, "assets/index-abc.css");

  // a_clean_bundle_on_a_current_sdk_passes
  it("a clean bundle has no finding", () => {
    expect(findings(":root{--brand-x:#123}")).toEqual([]);
  });

  // a_restated_token_fails_and_names_its_line: the summary is `<at> restates <subject>`.
  it("a restated token names its line", () => {
    const found = findings("a{}\n:root{--card:#fff}");
    expect(found).toHaveLength(1);
    expect(found[0].rule).toBe("restates-sdk-token");
    expect(found[0].at).toBe("assets/index-abc.css:2");
    expect(`${found[0].at} restates ${found[0].subject}`).toBe(
      "assets/index-abc.css:2 restates --card"
    );
  });

  // the_sdk_floor_fails_below_it_and_cannot_be_judged_without_a_version: its one theme finding.
  it("a theme finding is one finding, whatever the SDK version", () => {
    expect(findings(":root{--card:#fff}").map((f) => f.rule)).toEqual(["restates-sdk-token"]);
  });
});

describe("checkThemeCss, `built` unset: built CSS reads as it always did", () => {
  it("reports Tailwind's compiled theme layer", () => {
    expect(sourceRules(TAILWIND_THEME_LAYER)).toEqual([
      "1 restates-sdk-token --font-sans",
      "1 restates-sdk-token --radius-sm",
      "1 restates-sdk-token --color-card"
    ]);
  });

  it("reports the SDK's own blocks", () => {
    expect(sourceRules(SDK_BLOCKS_MINIFIED)).toEqual([
      "1 own-color-scheme color-scheme",
      "1 restates-sdk-token --oxy-theme-contract",
      "1 restates-sdk-token --scrim-blur",
      "1 restates-sdk-token --radius-control",
      "1 restates-sdk-token --font-sans",
      "1 restates-sdk-token --card",
      "1 restates-sdk-token --select-chevron",
      "1 own-mode-block :where(:root.dark,:root[data-oxy-mode=dark])",
      "1 bare-mode-class :where(:root.dark,:root[data-oxy-mode=dark])",
      "1 own-color-scheme color-scheme",
      "1 restates-sdk-token --background",
      "1 restates-sdk-token --card",
      "1 restates-sdk-token --scrim-blur",
      "1 restates-sdk-token --input",
      "1 restates-sdk-token --radius-sm",
      "1 restates-sdk-token --oxy-duration-fast",
      "1 restates-sdk-token --oxy-ease-out",
      "1 bare-mode-class .dark .oxy-shell-scope",
      "1 bare-mode-class .oxy-shell-scope.dark"
    ]);
  });

  it("reads an escaped class and a variant's :is(.dark *) as .dark", () => {
    expect(sourceRules(ESCAPED_DARK_CLASSES)).toEqual([
      String.raw`1 bare-mode-class .dark\:bg-card`,
      String.raw`1 bare-mode-class .w-1\.dark`
    ]);
    expect(sourceRules(DARK_VARIANT_UTILITIES)).toEqual([
      String.raw`1 bare-mode-class .dark\:text-white:is(.dark *)`,
      String.raw`1 bare-mode-class .dark\:text-white:is(.dark *)`,
      String.raw`2 bare-mode-class .dark\:bg-card:is(.dark *)`,
      String.raw`2 bare-mode-class .dark\:bg-card:is(.dark *)`,
      String.raw`3 bare-mode-class .dark\:bg-card:where(.dark,.dark *)`,
      String.raw`3 bare-mode-class .dark\:bg-card:where(.dark,.dark *)`,
      String.raw`3 bare-mode-class .dark\:bg-card:where(.dark,.dark *)`,
      String.raw`4 bare-mode-class .hover\:dark\:bg-muted:hover:is(.dark *)`,
      String.raw`5 bare-mode-class .md\:dark\:hover\:bg-muted:is(.dark *):hover`,
      String.raw`6 bare-mode-class .group-hover\:dark\:text-white:is(:where(.group):hover *):is(.dark *)`,
      String.raw`7 bare-mode-class .\32 xl\:dark\:bg-card:is(.dark *)`,
      String.raw`8 bare-mode-class .light\:bg-card:is(.light *)`,
      String.raw`8 bare-mode-class .light\:bg-card:is(.light *)`
    ]);
  });

  it("`built: false` is `built` unset", () => {
    for (const css of [
      TAILWIND_THEME_LAYER,
      APP_THEME_COMPILED,
      SDK_BLOCKS_MINIFIED,
      DARK_VARIANT_UTILITIES
    ])
      expect(checkThemeCss([{ file: "app.css", css }], { built: false })).toEqual(
        checkThemeCss([{ file: "app.css", css }])
      );
  });
});

describe("themeGuardSdkRules.gen.json", () => {
  it("is what the SDK's stylesheets declare (regenerate: `pnpm gen:theme-guard-rules`)", () => {
    expect(
      committedSdkRules,
      "stale: run `pnpm --filter @oxy-hq/sdk gen:theme-guard-rules` and commit the file"
    ).toEqual(generateSdkRules());
  });

  it("reads the stylesheets an app's bundle can carry", () => {
    const sheets = sdkStylesheets();
    expect(sheets.slice(0, 4)).toEqual([
      "src/theme/theme.css",
      "src/motion/motion.css",
      "src/shell/shell.css",
      "src/ui/base.css"
    ]);
    for (const rel of sheets) expect(existsSync(join(pkg, rel)), rel).toBe(true);
    // Each of these is an SDK rule the guard would report in an app.
    const own = committedSdkRules as Record<string, string[]>;
    expect(own[":where(:root)"]).toContain("color-scheme");
    expect(own[":where(:root.dark,:root[data-oxy-mode=dark])"]).toContain("--card");
    expect(own[":root"]).toContain("--oxy-duration-fast");
    expect(own[".dark.oxy-shell-scope,.oxy-shell-scope.dark"]).toContain("--oxy-shell-background");
  });
});

/** What a module reaches: every file it imports, itself included, and every package named on the way. */
interface Reach {
  seen: Set<string>;
  outside: Set<string>;
}

/**
 * Follow the imports and re-exports of `file` into `reach`, through every relative one. A
 * relative specifier that names no file fails the test.
 */
function followImports(file: string, reach: Reach): void {
  if (reach.seen.has(file)) return;
  reach.seen.add(file);
  if (file.endsWith(".json")) return;
  const source = readFileSync(file, "utf8");
  for (const m of source.matchAll(/^\s*(?:import|export)\b(?:[^;]*?\bfrom)?\s*"([^"]+)"/gm)) {
    const spec = m[1];
    if (!spec.startsWith(".")) {
      reach.outside.add(spec);
      continue;
    }
    const base = resolve(dirname(file), spec);
    const hit = [base, `${base}.ts`, `${base}.tsx`, join(base, "index.ts")].find(
      (p) => existsSync(p) && statSync(p).isFile()
    );
    expect(hit, `${spec} from ${relative(pkg, file)}`).toBeDefined();
    if (hit) followImports(hit, reach);
  }
}

describe("@oxy-hq/sdk/theme-guard", () => {
  it("exports the guard the testing entry exports", () => {
    expect(Object.keys(entry).sort()).toEqual([
      "SDK_THEME_NAMES",
      "checkThemeCss",
      "formatThemeFindings"
    ]);
    expect(entry.checkThemeCss).toBe(testingEntry.checkThemeCss);
    expect(entry.formatThemeFindings).toBe(testingEntry.formatThemeFindings);
    expect(entry.SDK_THEME_NAMES).toBe(testingEntry.SDK_THEME_NAMES);
    expect(entry.checkThemeCss([{ file: "a.css", css: ":root{--card:#fff}" }])).toHaveLength(1);
  });

  // The entry is for a Node tool that bundles it: no React, no DOM, no test runner, and none of
  // the rest of `@oxy-hq/sdk/testing`. Every module it reaches is listed here.
  it("imports the guard and nothing else", () => {
    const seen = new Set<string>();
    const outside = new Set<string>();
    followImports(join(pkg, "src", "theme-guard.ts"), { seen, outside });
    expect([...seen].map((f) => relative(pkg, f)).sort()).toEqual([
      "src/testing/themeGuard.ts",
      "src/testing/themeGuardScan.ts",
      "src/testing/themeGuardSdkRules.gen.json",
      "src/testing/themeGuardSelectors.ts",
      "src/theme-guard.ts",
      "src/theme/contract/contract.json"
    ]);
    expect([...outside].sort()).toEqual(["node:fs", "node:path"]);
  });
});
