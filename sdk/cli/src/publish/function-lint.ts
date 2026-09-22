/**
 * The Oxy Functions lint `oxyc validate` and `oxyc publish` run over an app's
 * function sources: the "works locally, fails closed in prod" mistakes that a
 * bundle carries silently and the host refuses on the first call.
 *
 * THREE RULES, each mirroring a refusal the platform already makes:
 *
 * - `capability` — a `ctx.<area>.<op>` call whose manifest capability the
 *   function does not declare. `capabilities.ts` is the map; `host.rs`'s
 *   `FunctionCapabilities` is the truth it is held to.
 * - `isolate-global` — a Node or Web global the isolate does not provide.
 *   `internal-docs/customer-apps-functions.md`, "Limits & safety": the isolate
 *   is bare `deno_core`, so `TextEncoder`/`TextDecoder`, `Blob`, `File`,
 *   `FormData`, `crypto.subtle` and `Buffer` are `ReferenceError`s that no
 *   `tsc` catches — and `process`, a Node global the same passage rules out
 *   ("only `oxy_functions_ext` ops plus what `BOOTSTRAP_JS` defines"), which
 *   `runtime.rs`'s bootstrap does not define.
 * - `destinations` / `engine` / `customer-warehouse` — a `ctx.warehouse` /
 *   `ctx.tx` write to a database the function may not write, or an op the
 *   engine refuses by name. The allowlist half is answered here; the half that
 *   needs the database's engine is `function-engines.ts`, at publish.
 *
 * LEXICAL, NOT A PARSER, AND NOT THE AUTHORITY — the same footing as
 * `placement-scan.ts`, whose `maskJs` this reuses: comments, strings, template
 * literals and regex literals are blanked before anything is matched, a call
 * is only seen when written on the context by name (`ctx.storage.put(`, or
 * `run.ctx.storage.put(` — a `ctx` handed around in a struct is still the
 * context), and a source that cannot be read is reported as not linted rather
 * than guessed at. Where this and the host disagree, the host is right; a
 * false positive is what `oxyc publish --allow-function-lint` is for.
 *
 * FOLLOWS RELATIVE IMPORTS. The entry alone is often a thin router — the
 * platform canary's `canary.ts` imports every `ctx` call from `steps.ts` — so
 * `./x`, `./x.js` (the TS spelling of `./x.ts`) and `./dir` (its `index.*`) are
 * read too, inside the app directory only, each file once. A package import
 * is not followed: what `@oxy-hq/sdk` does is its own test suite's problem.
 */

import { readFileSync, statSync } from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";

import { capabilitiesFor, WRITE_OPS } from "./capabilities.js";
import { functionEntry, type PublishManifest } from "./manifest.js";
import type { PlacementIssue } from "./placement.js";
import { lineAt, literalString, maskJs, splitArgs } from "./placement-scan.js";

export type LintRule =
  | "capability"
  | "destinations"
  | "isolate-global"
  | "engine"
  | "customer-warehouse";

export interface FunctionLintIssue extends PlacementIssue {
  rule: LintRule;
  /** The function the file belongs to — the manifest entry the fix edits. */
  fn: string;
}

/** One `ctx.<member>(` call, with what the engine check needs to know. */
export interface CtxCall {
  fn: string;
  /** Relative to the app directory. */
  file: string;
  line: number;
  /** `storage.put`, `oltp.query`, `tx`. */
  member: string;
  /** As written, up to and including the `(`: `ctx.storage.put(`. */
  call: string;
  /** The first argument when it is a string literal — the database of a write. */
  database?: string;
}

export interface FunctionLintResult {
  issues: FunctionLintIssue[];
  /** `ctx.warehouse` / `ctx.tx` writes, for the engine check that needs the server. */
  writes: CtxCall[];
  /** What was not checked, and why — one line each, deduplicated by the caller. */
  skipped: string[];
  /**
   * Every source that was read, relative to the app directory, each once — so
   * a test claiming an app "lints clean" can prove the claim is not vacuous.
   */
  files: string[];
}

/**
 * A global the isolate does not provide, and what to reach for instead.
 *
 * `member` restricts the match to `<name>.` — `process.env`, `crypto.subtle` —
 * because a bare `process(` is an ordinary function name, and `crypto` itself
 * is not the complaint: `crypto.subtle` is what the docs rule out.
 */
interface AbsentGlobal {
  name: string;
  member?: string;
  instead: string;
}

/** `internal-docs/customer-apps-functions.md` § "Limits & safety", plus `process`. */
export const ABSENT_GLOBALS: readonly AbsentGlobal[] = [
  {
    name: "Buffer",
    instead:
      "for base64 use `btoa`/`atob` (strings) or `bytesToBase64`/`base64ToBytes` from " +
      "`@oxy-hq/sdk` (bytes); pass binary to `ctx.storage.put` / `ctx.email.send` as " +
      '`{ encoding: "base64" }`'
  },
  {
    name: "TextEncoder",
    instead: "pass strings as-is — `ctx.fetch`, `ctx.storage` and `ctx.email.send` take UTF-8 text"
  },
  {
    name: "TextDecoder",
    instead: '`ctx.fetch` decodes as UTF-8 by default; ask for `{ encoding: "base64" }` for bytes'
  },
  { name: "Blob", instead: "send a string or a base64 body: `ctx.fetch(url, { body, encoding })`" },
  { name: "File", instead: "send a string or a base64 body: `ctx.fetch(url, { body, encoding })`" },
  {
    name: "FormData",
    instead: "encode the form body yourself (`URLSearchParams`, or a multipart string)"
  },
  {
    name: "crypto",
    member: "subtle",
    instead: "use `ctx.crypto` — HMAC sign / verify and a constant-time compare"
  },
  { name: "process", member: "", instead: "read configuration from `ctx.env`" }
];

const MANIFEST = "oxy-app.json";

const isObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

// ── sources ──────────────────────────────────────────────────────────────────

interface SourceFile {
  /** Relative to the app directory, `/`-separated. */
  rel: string;
  source: string;
}

const EXTENSIONS = [".ts", ".tsx", ".mts", ".js", ".mjs", ".jsx"];
/** A bound on the walk, so a pathological tree cannot make `validate` hang. */
const MAX_FILES = 200;

/** The file `spec` names from `from`, inside `appDir`, or `undefined`. */
function resolveRelative(appDir: string, from: string, spec: string): string | undefined {
  if (!spec.startsWith("./") && !spec.startsWith("../")) return undefined;
  const base = resolve(dirname(from), spec);
  if (base !== appDir && !base.startsWith(appDir + sep)) return undefined;
  const candidates = [base];
  // `./x.js` is how a TS source spells `./x.ts` under bundler resolution.
  if (/\.[mc]?jsx?$/.test(base)) {
    candidates.push(base.replace(/\.([mc]?)js$/, ".$1ts"), base.replace(/\.jsx?$/, ".tsx"));
  }
  for (const ext of EXTENSIONS) candidates.push(base + ext);
  for (const ext of EXTENSIONS) candidates.push(join(base, `index${ext}`));
  for (const candidate of candidates) {
    if (candidate.endsWith(".d.ts")) continue;
    try {
      if (statSync(candidate).isFile()) return candidate;
    } catch {
      // Not this spelling; the next may exist.
    }
  }
  return undefined;
}

/**
 * A quoted specifier at `lastIndex` in the ORIGINAL source, after real
 * whitespace only. Sticky, and applied to the original rather than the masked
 * text on purpose: masking blanks the quotes too, so a `\s*` matched on the
 * masked code would swallow the whole specifier and land past it.
 */
const SPECIFIER_AT = /\s*(['"])((?:\\.|(?!\1)[^\\\n])*)\1/y;

/**
 * The keyword an import specifier follows, found on the MASKED code so a
 * commented-out import is not followed, and read from the original at the
 * same offset, which masking preserves. A keyword not followed by a quote —
 * `Array.from(xs)`, `import.meta`, `import { x }` before its `from` — reads
 * nothing and is skipped.
 */
const SPECIFIER_STARTS = [/\b(?:import|export)\b[^;]*?\bfrom\b/g, /\bimport\s*\(/g, /\bimport\b/g];

function importSpecifiers(source: string, masked: string): string[] {
  const found: string[] = [];
  for (const re of SPECIFIER_STARTS) {
    for (const m of masked.matchAll(re)) {
      SPECIFIER_AT.lastIndex = (m.index ?? 0) + m[0].length;
      const spec = SPECIFIER_AT.exec(source);
      if (spec?.[2] !== undefined) found.push(spec[2]);
    }
  }
  return found;
}

/**
 * One app's sources, read once each however many functions reach them.
 *
 * Shared across an app's functions so a helper two entries import is read
 * once and — for the per-file rule, `isolate-global` — reported once. The
 * per-function rules still run per function: which capability an entry lacks
 * is that entry's business.
 */
interface SourceCache {
  loaded: Map<string, SourceFile>;
  unreadable: Map<string, string>;
}

/**
 * The entry and every source it reaches through relative imports.
 *
 * A file that cannot be read lands in `unreadable` with its code, and only the
 * ENTRY's absence is worth a line to the author — a helper that vanished mid
 * edit is the bundler's report to make.
 */
function loadSources(
  appDir: string,
  entry: string,
  cache: SourceCache
): { files: SourceFile[]; unreadable: Array<{ rel: string; code: string }> } {
  const files: SourceFile[] = [];
  const unreadable: Array<{ rel: string; code: string }> = [];
  const seen = new Set<string>();
  const queue = [resolve(appDir, entry)];
  while (queue.length > 0 && files.length < MAX_FILES) {
    const path = queue.shift() as string;
    if (seen.has(path)) continue;
    seen.add(path);
    const rel = relative(appDir, path).split(sep).join("/");
    let file = cache.loaded.get(path);
    if (file === undefined) {
      const known = cache.unreadable.get(path);
      if (known !== undefined) {
        unreadable.push({ rel, code: known });
        continue;
      }
      try {
        file = { rel, source: readFileSync(path, "utf8") };
        cache.loaded.set(path, file);
      } catch (cause) {
        const code = (cause as NodeJS.ErrnoException).code ?? "EUNKNOWN";
        cache.unreadable.set(path, code);
        unreadable.push({ rel, code });
        continue;
      }
    }
    files.push(file);
    for (const spec of importSpecifiers(file.source, maskJs(file.source))) {
      const next = resolveRelative(appDir, path, spec);
      if (next !== undefined && !seen.has(next)) queue.push(next);
    }
  }
  return { files, unreadable };
}

// ── ctx calls ────────────────────────────────────────────────────────────────

/**
 * `ctx.<area>(` or `ctx.<area>.<op>(`, optional-chained or not. `ctx` may be a
 * member of something (`run.ctx.`) but not the tail of a longer name
 * (`myctx.`), and the last segment must be CALLED — `ctx.airhouse.schema` is a
 * property read, which the host never sees.
 */
const CTX_CALL =
  /(?<![\w$])ctx\s*\??\.\s*([A-Za-z_$][\w$]*)(?:\s*\??\.\s*([A-Za-z_$][\w$]*))?\s*\(/g;

function ctxCalls(fn: string, file: SourceFile, masked: string): CtxCall[] {
  const calls: CtxCall[] = [];
  for (const m of masked.matchAll(CTX_CALL)) {
    const member = m[2] === undefined ? (m[1] as string) : `${m[1]}.${m[2]}`;
    const at = m.index ?? 0;
    const call: CtxCall = {
      fn,
      file: file.rel,
      line: lineAt(file.source, at),
      member,
      call: `ctx.${member}(`
    };
    if (WRITE_OPS.includes(member)) {
      const first = splitArgs(masked, at + m[0].length)?.[0];
      if (first) {
        const database = literalString(file.source.slice(first.start, first.end).trim());
        if (database !== undefined) call.database = database;
      }
    }
    calls.push(call);
  }
  return calls;
}

// ── isolate globals ──────────────────────────────────────────────────────────

const escapeRe = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/**
 * Does this file declare `name` itself — a `const`, a `function`, an `import`,
 * a parameter? Then a use of it is the author's own value, not the missing
 * global, and reporting it would be a false refusal at exit 1: the one thing
 * this lint must never do. So the check errs WIDE — a shadow anywhere in the
 * file silences the name for the whole file, block scope be damned — and what
 * a miss here costs is a hit, never a refusal.
 *
 * Type-only imports do not count: `import type { Blob }` gives a name to
 * annotate with, not one to call, so `new Blob(` still throws.
 */
function isShadowed(masked: string, name: string): boolean {
  const n = escapeRe(name);
  const declared = new RegExp(
    `\\b(?:const|let|var)\\s+(?:${n}\\b|\\{[^}]*\\b${n}\\b[^}]*\\}|\\[[^\\]]*\\b${n}\\b[^\\]]*\\])` +
      `|\\b(?:function|class)\\s+${n}\\b` +
      `|\\bimport\\s+(?!type\\b)[^;]*?\\b${n}\\b[^;]*?\\bfrom\\b` +
      `|\\bimport\\s+(?!type\\b)(?:\\*\\s+as\\s+)?${n}\\b` +
      // A single arrow parameter without parentheses: `Buffer => …`.
      `|(?<![\\w$.])${n}\\s*=>`
  );
  return declared.test(masked) || parameterLists(masked).some((list) => bindsName(list, n));
}

/**
 * Every parenthesised parameter list: after `function`, before `=>`, and a
 * method's `name(…) {`. Lexical, so `if (x) {` reads as a method too — which
 * only ever widens the shadow, never a refusal.
 */
function parameterLists(masked: string): string[] {
  const lists: string[] = [];
  const shapes = [
    /\bfunction\b[^(]*\(([^()]*)\)/g,
    /\(([^()]*)\)\s*(?::[^=]*?)?=>/g,
    /\b[\w$]+\s*\(([^()]*)\)\s*(?::[^{]*?)?\{/g
  ];
  for (const re of shapes) for (const m of masked.matchAll(re)) lists.push(m[1] ?? "");
  return lists;
}

/**
 * Does `list` BIND `name` — as a parameter, not as a type (`x: Buffer`,
 * `Array<Buffer>`, `Blob | File`) or a default-value use (`w = Buffer.from(…)`)?
 */
function bindsName(list: string, n: string): boolean {
  return new RegExp(`(?<![:<|&\\w$.]\\s*)\\b${n}\\b(?!\\s*[.(])`).test(list);
}

/**
 * The shape that throws. A VALUE use — called, constructed, or a member read —
 * never a type annotation (`body: Blob`), a `typeof` guard, or a property of
 * something else (`node.Buffer`). For a `member` global only `<name>.<member>`
 * counts, and `process.` matches any member.
 */
function absentGlobalShape(global: AbsentGlobal): RegExp {
  const n = escapeRe(global.name);
  if (global.member === undefined) return new RegExp(`(?<![\\w$.])(?:new\\s+)?${n}\\s*[.(]`, "g");
  const member = global.member === "" ? "[A-Za-z_$][\\w$]*" : `${escapeRe(global.member)}\\b`;
  return new RegExp(`(?<![\\w$.])${n}\\s*\\.\\s*${member}`, "g");
}

function absentGlobals(fn: string, file: SourceFile, masked: string): FunctionLintIssue[] {
  const issues: FunctionLintIssue[] = [];
  for (const global of ABSENT_GLOBALS) {
    if (isShadowed(masked, global.name)) continue;
    for (const m of masked.matchAll(absentGlobalShape(global))) {
      const written = m[0].replace(/\s+/g, "").replace(/^new/, "new ");
      const what = global.member ? `${global.name}.${global.member}` : global.name;
      issues.push({
        rule: "isolate-global",
        fn,
        level: "error",
        file: file.rel,
        path: `line ${lineAt(file.source, m.index ?? 0)}`,
        message:
          `\`${written}\` — the Oxy Functions isolate has no \`${what}\` (it is bare deno_core, ` +
          `not Node or a browser), so this throws ReferenceError at runtime; ${global.instead}`
      });
    }
  }
  return issues;
}

// ── capabilities and destinations ────────────────────────────────────────────

function capabilityIssues(spec: Record<string, unknown>, call: CtxCall): FunctionLintIssue[] {
  const issues: FunctionLintIssue[] = [];
  for (const cap of capabilitiesFor(call.member)) {
    if (cap.gate !== "manifest" || cap.declares(spec)) continue;
    issues.push({
      rule: "capability",
      fn: call.fn,
      level: "error",
      file: call.file,
      path: `line ${call.line}`,
      message:
        `\`${call.call}\` needs the \`${cap.manifest}\` capability, which function ` +
        `\`${call.fn}\` does not declare — the host refuses the call (fail-closed); add ` +
        `${cap.declare} to functions.${call.fn} in ${MANIFEST}`
    });
  }
  return issues;
}

/**
 * The allowlist half of `check_write_destination`: a write needs the database
 * in `destinations`, and an empty allowlist denies every database. The kind of
 * store, and the engine, wait for the server.
 */
function destinationIssue(
  spec: Record<string, unknown>,
  call: CtxCall
): FunctionLintIssue | undefined {
  if (!WRITE_OPS.includes(call.member)) return undefined;
  const destinations = Array.isArray(spec.destinations)
    ? spec.destinations.filter((d): d is string => typeof d === "string")
    : [];
  const written = call.database === undefined ? call.call : `${call.call}"${call.database}", …)`;
  const at = `functions.${call.fn}.destinations`;
  if (destinations.length === 0) {
    return {
      rule: "destinations",
      fn: call.fn,
      level: "error",
      file: call.file,
      path: `line ${call.line}`,
      message:
        `\`${written}\` is a write, and function \`${call.fn}\` declares no \`destinations\` — ` +
        `an empty allowlist refuses every database; add "destinations": ["<database>"] to ` +
        `functions.${call.fn} in ${MANIFEST}`
    };
  }
  if (call.database !== undefined && !destinations.includes(call.database)) {
    return {
      rule: "destinations",
      fn: call.fn,
      level: "error",
      file: call.file,
      path: `line ${call.line}`,
      message:
        `\`${written}\` writes \`${call.database}\`, which is not in \`${at}\` ` +
        `(${JSON.stringify(destinations)}) — the host refuses a write outside the allowlist; ` +
        `add it there in ${MANIFEST}`
    };
  }
  return undefined;
}

// ── entry point ──────────────────────────────────────────────────────────────

/**
 * Every lint issue in one app's functions, plus what the server must finish.
 *
 * `skipped` is worded for the author: which function was not read, and that
 * the engine check waits for `oxyc publish`. Nothing here needs a credential.
 */
export function lintAppFunctions(
  appDir: string,
  manifest: Record<string, unknown>
): FunctionLintResult {
  const result: FunctionLintResult = { issues: [], writes: [], skipped: [], files: [] };
  const cache: SourceCache = { loaded: new Map(), unreadable: new Map() };
  // `isolate-global` is a fact about a FILE, not about the function that
  // reached it: scanned once, reported once, credited to the first function.
  const globalsScanned = new Set<string>();
  const functions = isObject(manifest.functions) ? manifest.functions : {};
  for (const [fn, spec] of Object.entries(functions)) {
    if (!isObject(spec) || (spec.entry != null && typeof spec.entry !== "string")) continue;
    const entry = functionEntry(manifest as PublishManifest, fn);
    const { files, unreadable } = loadSources(appDir, entry, cache);
    const missingEntry = unreadable.find((u) => u.rel === entry.replace(/^\.\//, ""));
    if (missingEntry) {
      result.skipped.push(
        `function \`${fn}\` was not linted: ${entry} could not be read (${missingEntry.code})`
      );
      continue;
    }
    for (const file of files) {
      const masked = maskJs(file.source);
      if (!globalsScanned.has(file.rel)) {
        globalsScanned.add(file.rel);
        result.files.push(file.rel);
        result.issues.push(...absentGlobals(fn, file, masked));
      }
      for (const call of ctxCalls(fn, file, masked)) {
        result.issues.push(...capabilityIssues(spec, call));
        const destination = destinationIssue(spec, call);
        if (destination) result.issues.push(destination);
        else if (WRITE_OPS.includes(call.member)) result.writes.push(call);
      }
    }
  }
  result.issues.sort((a, b) => a.file.localeCompare(b.file) || lineOf(a) - lineOf(b));
  return result;
}

const lineOf = (issue: PlacementIssue) => Number(issue.path.replace(/^line /, "")) || 0;

/** `[function-lint/<rule>] file (line N): message` — the shape a warning prints. */
export function describeLintIssue(issue: FunctionLintIssue, prefix = ""): string {
  return `[function-lint/${issue.rule}] ${prefix}${issue.file} (${issue.path}): ${issue.message}`;
}
