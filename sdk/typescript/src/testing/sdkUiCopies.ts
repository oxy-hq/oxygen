// `checkSdkUiCopies(srcDir)`: the no-copy guard, for an app's own tests. An app takes the shared
// desk UI from `@oxy-hq/sdk/ui` and never keeps a copy of a piece the SDK ships: a copy looks
// right on the day it is made and then drifts, as the SDK's piece moves with the design system (a
// token, a focus ring, light and dark, a phone layout) and the copy stays where it was. The same
// check as the starter templates' `scripts/sdk-ui-guard.mjs` (oxygen-internal #4251), with its
// list of names taken from the ui entry itself (`SDK_UI_PIECES`), so it never drifts either.
//
// It reads the source files under `srcDir` (tests, `.d.ts` and `node_modules` left out) and finds
// every place a file DEFINES or EXPORTS a component under the name of an SDK piece:
// `function SidePanel`, `const Panel =`, `class Modal`, `export { Mine as LoadError }`. Importing
// the SDK's own, or re-exporting it from `@oxy-hq/sdk/...`, is fine. A deliberate local variant,
// kept on purpose, says why in the file that defines it:
//
//   // oxy-ui-local: SidePanel -- docks left as well as right, which the SDK's does not
//
// One line may name several pieces (`oxy-ui-local: Stat, StatGrid -- ...`). The reason after
// `--` is required: a line without one is a finding of its own. Comments and string contents
// are blanked before the definitions are read, keeping lines, so a piece named in a comment is
// not a copy. Node only (it reads files).

import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { isAbsolute, join, relative, resolve, sep } from "node:path";
import { SDK_UI_PIECES } from "./sdkUiPieces";

export { SDK_UI_PIECES } from "./sdkUiPieces";

export type SdkUiCopyKind = "copy" | "unexplained";

export interface SdkUiCopyFinding {
  /** The file, relative to `cwd`, with `/` separators. */
  file: string;
  /** 1-based line of the definition (or of the allow-list line). */
  line: number;
  /** `file:line`. */
  at: string;
  /** The SDK piece's name (the names an unexplained allow-list line gives, comma-joined). */
  name: string;
  /** `copy`: the file defines a piece the SDK ships. `unexplained`: an allow-list line with no
   *  reason after `--`. */
  kind: SdkUiCopyKind;
  /** The import that replaces a copy: `import { Panel } from "@oxy-hq/sdk/ui";`. */
  use: string;
  /** The allow-list line that keeps a deliberate variant, to fill in with the reason. */
  allow: string;
  message: string;
}

export interface CheckSdkUiCopiesOptions {
  /** Where a relative `srcDir` resolves, and what findings' paths are relative to. Default:
   *  `process.cwd()`. */
  cwd?: string;
  /** The names to guard. Default: every piece `@oxy-hq/sdk/ui` ships (`SDK_UI_PIECES`). */
  names?: Iterable<string>;
}

const SOURCE = /\.(?:[cm]?[jt]sx?)$/;
const SKIPPED = /\.(?:test|spec)\.[cm]?[jt]sx?$|\.d\.[cm]?ts$/;
// Within one line only: `[ \t]`, never `\s`, which would cross the newline and let a bare `--`
// take the next line of code as its reason.
const ALLOW = /oxy-ui-local:[ \t]*([A-Za-z0-9_$, \t]*?)[ \t]*(?:--[ \t]*(.*))?$/gm;

/** Comments and string contents replaced by spaces, newlines kept, so an index gives its line. */
function blankComments(text: string): string {
  let out = "";
  let i = 0;
  let quote: string | null = null;
  while (i < text.length) {
    const c = text[i];
    const next = text[i + 1];
    if (quote) {
      if (c === "\\") {
        out += "  ";
        i += 2;
        continue;
      }
      if (c === quote || (c === "\n" && quote !== "`")) {
        quote = null;
        out += c;
      } else out += c === "\n" ? "\n" : " ";
      i += 1;
    } else if (c === '"' || c === "'" || c === "`") {
      quote = c;
      out += c;
      i += 1;
    } else if (c === "/" && next === "/") {
      while (i < text.length && text[i] !== "\n") {
        out += " ";
        i += 1;
      }
    } else if (c === "/" && next === "*") {
      const end = text.indexOf("*/", i + 2);
      const stop = end < 0 ? text.length : end + 2;
      out += text.slice(i, stop).replace(/[^\n]/g, " ");
      i = stop;
    } else {
      out += c;
      i += 1;
    }
  }
  return out;
}

/** Every `{ name, at }` the code declares, or exports under, an SDK piece's name. */
function definitions(code: string, original: string, pieces: ReadonlySet<string>) {
  const found: { name: string; at: number }[] = [];
  const declared =
    /\b(?:function\s*\*?|class)\s+([A-Z][A-Za-z0-9_$]*)|\b(?:const|let|var)\s+([A-Z][A-Za-z0-9_$]*)\s*[:=]/g;
  for (const m of code.matchAll(declared)) {
    const name = m[1] ?? m[2];
    if (name && pieces.has(name)) found.push({ name, at: m.index ?? 0 });
  }
  // The module a re-export names is a string, blanked in `code`: read it from the original.
  const exported = /\bexport\s+(?:type\s+)?\{([^}]*)\}(\s*from\s*)?/g;
  for (const m of code.matchAll(exported)) {
    const at = m.index ?? 0;
    const from = m[2]
      ? /^\s*(["'])([^"']*)\1/.exec(original.slice(at + m[0].length))?.[2]
      : undefined;
    if (from?.startsWith("@oxy-hq/sdk")) continue;
    for (const spec of m[1].split(",")) {
      const alias = /\bas\s+([A-Za-z0-9_$]+)\s*$/.exec(spec.trim());
      // `export { Panel }` names a local the declaration pattern already reported.
      if (alias && pieces.has(alias[1])) found.push({ name: alias[1], at });
    }
  }
  return found;
}

/** The names a file allow-lists, and its allow-list lines that give no reason. */
function allowances(text: string) {
  const allowed = new Set<string>();
  const unexplained: { names: string[]; at: number }[] = [];
  for (const m of text.matchAll(ALLOW)) {
    const names = m[1].split(/[\s,]+/).filter(Boolean);
    if (!m[2]?.trim()) {
      unexplained.push({ names, at: m.index ?? 0 });
      continue;
    }
    for (const n of names) allowed.add(n);
  }
  return { allowed, unexplained };
}

function sourceFiles(dir: string): string[] {
  let entries: import("node:fs").Dirent[];
  try {
    entries = readdirSync(dir, { withFileTypes: true });
  } catch {
    return [];
  }
  return entries.flatMap((e) => {
    const path = join(dir, e.name);
    if (e.isDirectory()) return e.name === "node_modules" ? [] : sourceFiles(path);
    return SOURCE.test(e.name) && !SKIPPED.test(e.name) ? [path] : [];
  });
}

const lineAt = (text: string, at: number) => text.slice(0, at).split("\n").length;

const importFor = (name: string) => `import { ${name} } from "@oxy-hq/sdk/ui";`;
const allowLineFor = (name: string) => `// oxy-ui-local: ${name} -- <why the SDK's does not fit>`;

/**
 * Every copied SDK piece under `srcDir` (default `"src"`), and every allow-list line with no
 * reason, sorted by file and line; `[]` when there are none.
 */
export function checkSdkUiCopies(
  srcDir = "src",
  options: CheckSdkUiCopiesOptions = {}
): SdkUiCopyFinding[] {
  const cwd = options.cwd ?? process.cwd();
  const pieces = new Set(options.names ?? SDK_UI_PIECES);
  const root = isAbsolute(srcDir) ? srcDir : resolve(cwd, srcDir);
  // A guard rooted at the wrong place would read nothing and pass, so a missing root throws.
  if (!existsSync(root) || !statSync(root).isDirectory()) {
    throw new Error(`checkSdkUiCopies: no source directory at ${root}`);
  }
  const out: SdkUiCopyFinding[] = [];
  for (const path of sourceFiles(root)) {
    const text = readFileSync(path, "utf8");
    const file = relative(cwd, path).split(sep).join("/");
    const { allowed, unexplained } = allowances(text);
    for (const u of unexplained) {
      const line = lineAt(text, u.at);
      const name = u.names.join(", ");
      out.push({
        file,
        line,
        at: `${file}:${line}`,
        name,
        kind: "unexplained",
        use: importFor(name),
        allow: allowLineFor(name),
        message: `allow-lists ${name} without a reason: say why after "--"`
      });
    }
    for (const d of definitions(blankComments(text), text, pieces)) {
      if (allowed.has(d.name)) continue;
      const line = lineAt(text, d.at);
      out.push({
        file,
        line,
        at: `${file}:${line}`,
        name: d.name,
        kind: "copy",
        use: importFor(d.name),
        allow: allowLineFor(d.name),
        message: `defines ${d.name}, which @oxy-hq/sdk/ui ships: use the SDK's`
      });
    }
  }
  // Code-unit order, the same on every machine (not the locale's).
  return out.sort((a, b) => (a.file < b.file ? -1 : a.file > b.file ? 1 : a.line - b.line));
}

/** One paragraph per finding: what to import instead, and how to keep a deliberate variant. */
export function formatSdkUiCopies(findings: readonly SdkUiCopyFinding[]): string {
  return findings
    .map((f) =>
      f.kind === "unexplained"
        ? `${f.at} ${f.message}:\n  ${f.allow}`
        : `${f.at} ${f.message}:\n  ${f.use}\n  A deliberate local variant? Keep it with a line in that file saying why:\n  ${f.allow}`
    )
    .join("\n\n");
}
