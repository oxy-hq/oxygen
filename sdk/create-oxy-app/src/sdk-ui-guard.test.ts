// The templates' no-copy check, held here where it runs on every create-oxy-app test without a
// scaffold or an install. The check itself is the SDK's, `checkSdkUiCopies()` from
// `@oxy-hq/sdk/testing` (its grammar is tested in sdk/typescript/src/testing/sdkUiCopies.test.ts,
// its list of names in sdkUiPieces.test.ts). What this file holds is that every template runs it
// and that it bites on each template's own `src/`:
//
//   * a planted `src/components/SidePanel.tsx` fails, naming the import to use instead; the same
//     file allow-listed with a reason passes; removed, it passes;
//   * every template's `tests/design-system.test.ts` takes it (and `checkThemeCss()`) from
//     `@oxy-hq/sdk/testing`, the four are the same file, none ships a local copy of the check,
//     and each asks for an SDK that has it.
//
// It runs this repo's SDK source, not the published package, as `scripts/ci/verify-templates.mjs`
// links the local SDK: the templates pin what the next publish produces. That script is the
// end-to-end proof, a scaffold's own `pnpm test` failing on the same plant.

import {
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { afterEach, describe, expect, test } from "vitest";

interface SdkUiCopyFinding {
  at: string;
  name: string;
  kind: "copy" | "unexplained";
}
interface SdkUiCopies {
  checkSdkUiCopies(srcDir?: string, options?: { cwd?: string }): SdkUiCopyFinding[];
  formatSdkUiCopies(findings: readonly SdkUiCopyFinding[]): string;
}

const PKG = resolve(import.meta.dirname, "..");
const TEMPLATES = join(PKG, "templates");
// Imported by path, its shape restated above: this package's tsconfig roots at `src/`, so a
// static import of the SDK's source would pull a file outside it into the typecheck.
const SDK_UI_COPIES = resolve(PKG, "../typescript/src/testing/sdkUiCopies.ts");
const { checkSdkUiCopies, formatSdkUiCopies } = (await import(SDK_UI_COPIES)) as SdkUiCopies;

// The first @oxy-hq/sdk that ships `checkSdkUiCopies`.
const SDK_FLOOR = [2, 31, 0];

const PLANT = "src/components/SidePanel.tsx";
const COPY = "export function SidePanel() {\n  return null;\n}\n";
const ALLOWED = `// oxy-ui-local: SidePanel -- a planted variant, to prove the allow-list\n${COPY}`;

const ids = readdirSync(TEMPLATES, { withFileTypes: true })
  .filter((e) => e.isDirectory())
  .map((e) => e.name)
  .sort();
const TEST_FILE = "tests/design-system.test.ts";
const reference = readFileSync(join(TEMPLATES, "vite", TEST_FILE), "utf8");

let root: string | undefined;
afterEach(() => {
  if (root) rmSync(root, { recursive: true, force: true });
  root = undefined;
});

test("the templates are found", () => {
  expect(ids).toEqual(expect.arrayContaining(["dashboard", "functions", "single-store", "vite"]));
});

describe.each(ids)("the %s template", (id) => {
  test("its src/ fails a planted SidePanel, passes it allow-listed, and passes it removed", () => {
    root = mkdtempSync(join(tmpdir(), `sdk-ui-guard-${id}-`));
    cpSync(join(TEMPLATES, id, "src"), join(root, "src"), { recursive: true });
    const cwd = root;
    const check = () => checkSdkUiCopies("src", { cwd });
    const plant = join(root, PLANT);
    mkdirSync(dirname(plant), { recursive: true });

    writeFileSync(plant, COPY);
    const found = check();
    expect(found.map((f) => `${f.at} ${f.kind} ${f.name}`)).toEqual([`${PLANT}:1 copy SidePanel`]);
    expect(formatSdkUiCopies(found)).toContain('import { SidePanel } from "@oxy-hq/sdk/ui";');

    writeFileSync(plant, ALLOWED);
    expect(formatSdkUiCopies(check())).toBe("");

    rmSync(plant);
    expect(formatSdkUiCopies(check())).toBe("");
  });

  test("its pnpm test runs the SDK's checks, and it carries no copy of them", () => {
    const testFile = readFileSync(join(TEMPLATES, id, TEST_FILE), "utf8");
    expect(testFile).toBe(reference);
    const imported = /import\s*\{([^}]*)\}\s*from\s*"@oxy-hq\/sdk\/testing"/.exec(testFile)?.[1];
    for (const name of ["checkSdkUiCopies", "formatSdkUiCopies", "checkThemeCss"]) {
      expect(imported?.split(",").map((s) => s.trim())).toContain(name);
    }
    expect(existsSync(join(TEMPLATES, id, "scripts/sdk-ui-guard.mjs"))).toBe(false);

    const pkg = JSON.parse(readFileSync(join(TEMPLATES, id, "package.json"), "utf8"));
    expect(pkg.scripts.test).toBe("vitest run");
    const range: string = pkg.dependencies["@oxy-hq/sdk"];
    expect(range).toMatch(/^\^\d+\.\d+\.\d+$/);
    const [major, minor, patch] = range.slice(1).split(".").map(Number);
    const atLeast =
      major > SDK_FLOOR[0] ||
      (major === SDK_FLOOR[0] &&
        (minor > SDK_FLOOR[1] || (minor === SDK_FLOOR[1] && patch >= SDK_FLOOR[2])));
    expect(atLeast, `${id} asks for @oxy-hq/sdk${range}, before checkSdkUiCopies`).toBe(true);
  });
});
