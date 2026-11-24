// This app stands on the shared design system: the SDK's theme and its desk UI.
//
// 1. Its CSS works with `@oxy-hq/sdk/theme.css`, never against it. The SDK's own guard reads
//    every stylesheet under `src/` and `index.html`'s <style>: no theme token restated, no
//    literal colour in a Tailwind `@theme`, no light/dark mode of the app's own.
// 2. It keeps no copy of a piece `@oxy-hq/sdk/ui` ships (`checkSdkUiCopies()`): a local
//    `SidePanel`, `LoadError` or `DefaultTabs` fails here, naming the import to use instead. A
//    variant kept on purpose says why in its file: `// oxy-ui-local: <Name> -- <why>`.
//
// Files are read by path, not with `?raw` imports, which Vitest turns into empty strings.

import { readdirSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import {
  checkSdkUiCopies,
  checkThemeCss,
  formatSdkUiCopies,
  formatThemeFindings
} from "@oxy-hq/sdk/testing";
import { describe, expect, it } from "vitest";

const ROOT = fileURLToPath(new URL("..", import.meta.url));

/** Every stylesheet under `src/`, as a path from the app's root. */
function stylesheets(dir = join(ROOT, "src")): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const path = join(dir, e.name);
    if (e.isDirectory()) return stylesheets(path);
    return e.name.endsWith(".css") ? [relative(ROOT, path)] : [];
  });
}

describe("the design system", () => {
  it("the CSS restates no SDK token and keeps no light/dark of its own", () => {
    const found = checkThemeCss([...stylesheets(), "index.html"], { cwd: ROOT });
    expect(formatThemeFindings(found)).toBe("");
  });

  it("src/ keeps no copy of an @oxy-hq/sdk/ui piece", () => {
    expect(formatSdkUiCopies(checkSdkUiCopies("src", { cwd: ROOT }))).toBe("");
  });
});
