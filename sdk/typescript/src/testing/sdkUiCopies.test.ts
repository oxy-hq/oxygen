// The no-copy guard, on a scratch app: a component defined (or exported) under an SDK piece's
// name is a finding with its file:line, the import to use and the allow-list line; the SDK's own
// import or re-export is not; a name in a comment or a string is not; a reasoned allow-list keeps
// a deliberate variant and an unreasoned one is a finding of its own; tests, `.d.ts` and
// node_modules are not read; and the format names what to do.

import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { afterAll, describe, expect, it } from "vitest";
import { checkSdkUiCopies, formatSdkUiCopies, SDK_UI_PIECES } from "./index";

const scratch = mkdtempSync(join(tmpdir(), "oxy-ui-copies-"));
afterAll(() => rmSync(scratch, { recursive: true, force: true }));

let apps = 0;
/** A scratch app with these files under it; its root. */
function app(files: Record<string, string>): string {
  apps += 1;
  const root = join(scratch, `app-${apps}`);
  for (const [path, text] of Object.entries(files)) {
    mkdirSync(dirname(join(root, path)), { recursive: true });
    writeFileSync(join(root, path), text);
  }
  return root;
}

const found = (root: string) =>
  checkSdkUiCopies("src", { cwd: root }).map((f) => `${f.at} ${f.kind} ${f.name}`);

describe("checkSdkUiCopies", () => {
  it("finds a piece defined as a function, a const, a class, or exported under its name", () => {
    const root = app({
      "src/chrome/Panel.tsx":
        "import type { ReactNode } from 'react';\n\nexport function Panel() {\n  return null;\n}\n",
      "src/chrome/bits.tsx":
        "export const Stat = () => null;\nlet Chip: unknown;\nclass Modal {}\n",
      "src/chrome/index.ts": "import { Mine } from './Mine';\nexport { Mine as LoadError };\n"
    });
    expect(found(root)).toEqual([
      "src/chrome/Panel.tsx:3 copy Panel",
      "src/chrome/bits.tsx:1 copy Stat",
      "src/chrome/bits.tsx:2 copy Chip",
      "src/chrome/bits.tsx:3 copy Modal",
      "src/chrome/index.ts:2 copy LoadError"
    ]);
  });

  it("says which import replaces a copy, and the line that keeps a deliberate one", () => {
    const root = app({ "src/Panel.tsx": "export function Panel() {}\n" });
    const [f] = checkSdkUiCopies("src", { cwd: root });
    expect(f.use).toBe('import { Panel } from "@oxy-hq/sdk/ui";');
    expect(f.allow).toBe("// oxy-ui-local: Panel -- <why the SDK's does not fit>");
    const text = formatSdkUiCopies([f]);
    expect(text).toContain("src/Panel.tsx:1 defines Panel, which @oxy-hq/sdk/ui ships");
    expect(text).toContain(f.use);
    expect(text).toContain(f.allow);
  });

  it("leaves the SDK's own alone: an import, a re-export from the SDK, other names", () => {
    const root = app({
      "src/a.tsx":
        'import { Panel, Stat } from "@oxy-hq/sdk/ui";\nexport { Panel, SidePanel as Sheet } from "@oxy-hq/sdk/ui";\nexport { Stat as Stat } from \'@oxy-hq/sdk/ui\';\nexport function Pane() {}\nconst PanelBody = 1;\nfunction usePanel() {}\n',
      "src/b.tsx":
        'export { Chip as Pill } from "./Chip";\nexport { Mine as StatusDot } from "./Mine";\n'
    });
    // A re-export from the app's own file under a piece's name is a copy all the same.
    expect(found(root)).toEqual(["src/b.tsx:2 copy StatusDot"]);
  });

  it("does not read a piece named in a comment or a string as a copy", () => {
    const root = app({
      "src/notes.ts":
        '// function Panel is the SDK\'s now\n/* class Modal {} */\nconst words = "function Stat() {}";\nconst more = `const Chip = 1`;\n'
    });
    expect(found(root)).toEqual([]);
  });

  it("keeps a deliberate variant with a reason, and reports an allow-list line with none", () => {
    const root = app({
      "src/kept.tsx":
        "// oxy-ui-local: SidePanel, Modal -- docks left as well as right, which the SDK's does not\nexport function SidePanel() {}\nexport function Modal() {}\n",
      "src/bare.tsx": "// oxy-ui-local: Stat\nexport function Stat() {}\n",
      "src/dashes.tsx": "// oxy-ui-local: Chip --\nexport const Chip = 1;\n"
    });
    expect(found(root)).toEqual([
      "src/bare.tsx:1 unexplained Stat",
      "src/bare.tsx:2 copy Stat",
      "src/dashes.tsx:1 unexplained Chip",
      "src/dashes.tsx:2 copy Chip"
    ]);
    const text = formatSdkUiCopies(checkSdkUiCopies("src", { cwd: root }));
    expect(text).toContain('src/bare.tsx:1 allow-lists Stat without a reason: say why after "--"');
  });

  it("reads source files only: no tests, no .d.ts, nothing under node_modules", () => {
    const root = app({
      "src/Panel.test.tsx": "function Panel() {}\n",
      "src/Stat.spec.ts": "function Stat() {}\n",
      "src/types.d.ts": "declare function Modal(): void;\nexport class Chip {}\n",
      "src/node_modules/x/index.js": "export function Panel() {}\n",
      "src/styles.css": ".Panel { }\n",
      "src/ok.mjs": "export function Panel() {}\n"
    });
    expect(found(root)).toEqual(["src/ok.mjs:1 copy Panel"]);
  });

  it("guards every piece the ui entry ships by default, or the names given", () => {
    expect(SDK_UI_PIECES).toContain("SidePanel");
    expect(SDK_UI_PIECES).toContain("TryAgainButton");
    const root = app({ "src/x.tsx": "export function Panel() {}\nexport function Widget() {}\n" });
    expect(found(root)).toEqual(["src/x.tsx:1 copy Panel"]);
    expect(
      checkSdkUiCopies(join(root, "src"), { cwd: root, names: ["Widget"] }).map((f) => f.name)
    ).toEqual(["Widget"]);
    // A misrooted guard must not pass by reading nothing.
    expect(() => checkSdkUiCopies("missing", { cwd: root })).toThrow(/no source directory/);
  });
});
