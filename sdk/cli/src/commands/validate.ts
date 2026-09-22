/**
 * `oxyc validate` — check a workspace's YAML against the schemas.
 *
 * ONE SOURCE OF TRUTH, WHICH IS THE ONLY REASON THIS IS HONEST TO PORT.
 * The schemas are not written here: `json-schemas/*.json` are generated from
 * the Rust config types by `oxy gen-config-schema`, and `crates/app`'s
 * `json_schemas_are_current` test fails if the committed copies drift from
 * those types. This reads them. A hand-written TypeScript mirror of
 * `AgentConfig` would be a second definition that diverges silently — the
 * exact shape of bug this CLI has spent its whole review finding.
 *
 * DOES NOT REPLACE `oxy validate`. That one builds a real `ConfigBuilder`, so
 * it can resolve `databases:` entries and `llm.ref` against `config.yml` —
 * checks that need the workspace loaded, not just parsed. This is the
 * structural half: schema conformance, fast, no Rust binary, and reachable
 * from `npx`. Where they disagree, the Rust one is right.
 */

import { existsSync, lstatSync, readdirSync, readFileSync, realpathSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, extname, join, parse, relative, resolve } from "node:path";
import { Ajv, type ErrorObject } from "ajv";
import addFormats from "ajv-formats";
import { parse as parseYaml } from "yaml";
import { lintAppFunctions } from "../publish/function-lint.js";
import { checkAppPlacement } from "../publish/placement.js";
import { REINSTALL_REMEDY } from "../template/embedded.js";
import { schemasDir } from "../template/locate.js";
import * as log from "../ui/log.js";
import { heading, table } from "../ui/render.js";
import { out } from "../ui/tty.js";
import { CliError, ExitCode } from "../util/errors.js";
import { repoRoot } from "../util/git.js";
import { inferOrgApp } from "./publish.js";

/**
 * Which schema governs which file.
 *
 * Keyed by the SUFFIX rather than the extension, because these are all `.yml`
 * and the kind is carried by the second-to-last segment. `.workflow.yml` is
 * the retired spelling of `.automation.yml` and is still accepted by the
 * platform, so it is accepted here.
 */
const SCHEMA_KINDS: ReadonlyArray<readonly [stem: string, schema: string]> = [
  [".automation", "workflow.json"],
  [".procedure", "workflow.json"],
  [".workflow", "workflow.json"],
  [".agentic", "agentic.json"],
  [".app", "app.json"],
  [".agent.test", "agent-test.json"],
  ["config", "config.json"]
];

/**
 * Both YAML spellings, because the walk accepts both and the product does too.
 *
 * The first version listed only `.yml`, so `config.yaml` and `x.app.yaml` were
 * walked, matched nothing, and skipped in silence — while the web app's Monaco
 * config validates both spellings. A workspace using `.yaml` would have been
 * reported clean without a single file being read.
 */
const SCHEMA_FOR: ReadonlyArray<readonly [suffix: string, schema: string]> = SCHEMA_KINDS.flatMap(
  ([stem, schema]) =>
    [
      [`${stem}.yml`, schema],
      [`${stem}.yaml`, schema]
    ] as const
);

/**
 * Directories never walked.
 *
 * A superset of the canonical list in `migrate_automations.rs` rather than a
 * copy of it — `build` is the one that matters and was missing, because a
 * stray rendered copy of a workspace file actually lands there. The extras
 * (`.github`, `out`, `__pycache__`) cost nothing: nothing validatable is
 * authored in them.
 */
const SKIP = new Set([
  "build",
  ".git",
  ".github",
  "node_modules",
  "target",
  "dist",
  "out",
  ".worktrees",
  ".oxy_state",
  ".venv",
  "__pycache__"
]);

/**
 * The custom-app manifest, checked for DATA PLACEMENT rather than against a
 * schema: none is generated for it — the SDK's type is its only definition —
 * so `publish/placement.ts` checks where the app's data goes instead.
 */
const APP_MANIFEST = "oxy-app.json";

/** Matched on the basename, for the reason `schemaFor` gives. */
function isAppManifest(path: string): boolean {
  return (path.split("\\").join("/").split("/").pop() ?? path) === APP_MANIFEST;
}

export interface ValidateFlags {
  /** Validate one file instead of the whole workspace. */
  file?: string;
  /** Emit findings as JSON. */
  json?: boolean;
}

interface Finding {
  file: string;
  path: string;
  message: string;
}

/** The schema a path is governed by, or `undefined` for a file we do not check. */
export function schemaFor(path: string): string | undefined {
  const normalised = path.split("\\").join("/");
  const basename = normalised.split("/").pop() ?? normalised;
  for (const [suffix, schema] of SCHEMA_FOR) {
    // MATCHED AGAINST THE BASENAME, not the whole path, and the distinction
    // is not cosmetic: `endsWith("config.yml")` is true for `myconfig.yml`,
    // which would then be validated against the workspace config schema and
    // reported as broken for having none of its fields. Same shape as
    // `/api/user` matching `/api/users` in the proxy.
    //
    // A dotted suffix still needs the leading `.`, so `notanapp.yml` does not
    // match `.app.yml`.
    if (suffix.startsWith(".") ? basename.endsWith(suffix) : basename === suffix) return schema;
  }
  return undefined;
}

/** What the walk found: files to check, and links it could not follow. */
interface Walked {
  files: string[];
  /**
   * Entries named like a workspace file that could not be read, and why.
   *
   * NOT ONLY LINKS. A FIFO, socket or device named `x.app.yml` lands here too,
   * and none of them is a symlink — the warning string and this doc both said
   * "symlink" while the bucket had already widened.
   */
  broken: SkippedFile<string>[];
}

/**
 * A file that was named and NOT checked, and the reason it was not.
 *
 * One shape, two buckets: `broken` for an entry the filesystem would not give
 * us, `unchecked` for one we had no schema to check it against. The code spaces
 * are different — `whyUnreadable` explains the first, `whyUnchecked` the second
 * — but a caller reading `--json` gets the same `{path, code}` either way,
 * which is what lets it act on the reason instead of on the bucket's name.
 *
 * The reason is CHECKED, not assumed. Reporting every `statSync` failure as
 * "the target is gone" was true for `ENOENT` and a claim about a condition
 * nobody established for the rest — `ELOOP` for `ln -s x.app.yml x.app.yml`
 * (worth naming, on a walk that deliberately avoids needing cycle detection),
 * `EACCES` for an unreadable directory above the target, `ENOTDIR` for a link
 * through a path component that is a file.
 *
 * `code` is the OS errno except for two that are OURS, said here so a caller
 * branching on it knows which names come from the kernel: `ENOTFILE`, because
 * there is no errno for "this is a FIFO", and `EUNKNOWN`, for the case where
 * the thrown error carried no code at all. Inventing either silently would be
 * the same defect as the assumed cause above.
 */
interface SkippedFile<C extends string> {
  path: string;
  code: C;
}

/**
 * The `--json` document's shape, declared once.
 *
 * An INTERFACE rather than a shared empty literal to spread. The spread bought
 * the same "both documents carry every field" property, but paid for it with a
 * silent default: add a field and the populated document emits its EMPTY value
 * beside real data. Annotating both literals makes the omission a compile error
 * instead, which is the version that cannot ship wrong.
 */
interface ValidateReport {
  checked: number;
  unchecked: SkippedFile<SkipCode>[];
  broken: SkippedFile<string>[];
  findings: Finding[];
}

/** Our own code for an entry that exists and is not a regular file. */
const ENOTFILE = "ENOTFILE";

/** Our own code for a throw that carried none — never an OS errno. */
const EUNKNOWN = "EUNKNOWN";

/** Named once: two throws print it, and one of them may replace it. */
const WHOLE_WORKSPACE_HINT = "oxyc validate            — to check the whole workspace";

/** The kind is known and its schema is not on disk in this installation. */
const SCHEMA_MISSING = "SCHEMA_MISSING";

/**
 * `oxyc` has no schema mapped for this file kind at all.
 *
 * UNREACHABLE THROUGH BOTH DOORS TODAY, on the same footing as the `undefined`
 * this replaced: `walk` filters on `!schemaFor(rel)` before it emits, and the
 * `--file` branch throws USAGE on the same predicate. It exists so the value
 * and the message agree with the contract the day either filter moves — which
 * is why `whyUnchecked` is pinned directly rather than through a run that
 * cannot produce this code.
 */
const KIND_UNKNOWN = "KIND_UNKNOWN";

/**
 * Every reason `unchecked` can hold — a CLOSED set, unlike an OS errno.
 *
 * Written as a union so `whyUnchecked` can be exhaustive rather than merely
 * typed: see the `never` at the end of it.
 */
type SkipCode = typeof SCHEMA_MISSING | typeof KIND_UNKNOWN;

/**
 * Why a file was skipped, and — separately — what to do about it.
 *
 * TWO FIELDS, because they belong in two places. Folded into one string, the
 * only reachable headline read `3 file(s) NOT checked — the schema is not in
 * this installation — reinstall, or set OXYC_SCHEMAS_DIR at a checkout's
 * json-schemas/`: 140 characters and two em dashes, of which the second half
 * is a remedy. `listSkipped` puts the reason on the count and the remedy on its
 * own line below the paths, through `log.remedy` — NOT `log.hint`, whose `→`
 * elaborates the line above and so reads in sequence with the paths. The
 * remedy is the one line that is about what to DO, and the blank lines around
 * it are what say so.
 */
interface SkipReason {
  reason: string;
  remedy?: string;
}

/**
 * Why a file was not checked.
 *
 * TWO PRODUCERS, TWO REMEDIES, and the message used to assert the first one's
 * for both: "reinstall, or set OXYC_SCHEMAS_DIR" is right for a schema absent
 * from the installation and wrong for a kind `oxyc` does not map, where no
 * reinstall produces one.
 */
export function whyUnchecked(code: SkipCode): SkipReason {
  if (code === SCHEMA_MISSING) {
    return {
      // Worded to read after both "3 file(s) NOT checked —" and "a0.app.yml —",
      // since `listSkipped` puts it in either place.
      reason: "the schema is not in this installation",
      remedy: "reinstall, or set OXYC_SCHEMAS_DIR at a checkout's json-schemas/"
    };
  }
  if (code === KIND_UNKNOWN) {
    // No remedy: no reinstall produces a mapping `oxyc` does not have. That
    // asymmetry is the whole reason the two producers stopped sharing a line.
    return { reason: "oxyc has no schema for this kind of file" };
  }

  // EXHAUSTIVE, not merely typed. The union alone rejects a bad LITERAL at a
  // call site and nothing more, so ADDING A THIRD MEMBER compiled and fell out
  // of the last arm wearing `KIND_UNKNOWN`'s sentence — a claim about a cause
  // nobody established, which is the assumed-cause defect the surrounding
  // commits exist to close. `KIND_UNKNOWN`'s own doc names a third member as
  // the growth path, so this is the reachable shape, not a hypothetical.
  //
  // NOT a typo'd constant VALUE, which an earlier version of this comment also
  // claimed: producer and consumer both reference the constant, so a mistyped
  // `KIND_UNKNOWN` still reaches the arm written for it and the reader still
  // gets the right sentence. One shape, and `never` closes it.
  //
  // What an impossible code does NOW is throw — `whyUnchecked` returns
  // `undefined`, and `listSkipped`'s per-file line dereferences it. Loud and
  // wrong beats quiet and wrong, which is the trade, but it is a change and
  // worth saying rather than implying nothing happens.
  //
  // `whyUnreadable` cannot have any of this: the kernel's errno space is open,
  // which is why it keeps a fallback and this does not.
  const unreachable: never = code;
  return unreachable;
}

/**
 * What each code actually means, for the hint beside the path.
 *
 * Exported for the same reason `whyUnchecked` is: two of these arms —
 * `EACCES`/`EPERM` and `EUNKNOWN` — cannot be produced by a run on CI without
 * `chmod` or root, so swapping their strings passed the whole suite. A run
 * exercises the other four.
 */
export function whyUnreadable(code: string): SkipReason {
  if (code === "ENOENT") return { reason: "the target is gone" };
  if (code === "ELOOP") return { reason: "the link points at itself, or round a cycle" };
  if (code === "EACCES" || code === "EPERM") {
    return { reason: "permission denied on the path to the target" };
  }
  if (code === "ENOTDIR") return { reason: "a component of the path is a file, not a directory" };
  if (code === ENOTFILE) return { reason: "not a regular file — a socket, device or FIFO" };
  if (code === EUNKNOWN) return { reason: "the filesystem refused it without saying why" };
  // The fallback `whyUnchecked` deliberately does not have — an errno nobody
  // mapped is a real possibility, and the code itself beats inventing a cause.
  return { reason: code };
}

/**
 * Every YAML file under `root` that a schema governs.
 *
 * `withFileTypes` hands back a `Dirent`, which does NOT follow links, so every
 * link — to a file, to a directory, to nothing — answers `false` to
 * `isDirectory()` and falls through to the extension check together. The three
 * are separated here, by one `statSync` per link that reaches it:
 *
 * - A link to a FILE is emitted under its OWN name, and `schemaFor` decides its
 *   kind from that name. `--file` was changed back to match — in an Oxy
 *   workspace the kind of a file is its name, and a link called `x.app.yml` is
 *   an app.
 * - A link to a DIRECTORY is skipped whatever it is called. Not descended into,
 *   deliberately: following one needs cycle detection, and this is the cheap way
 *   not to need it. Skipped EXPLICITLY rather than by omission, because a name
 *   like `apps.app.yml` clears the extension check and reaches `readFileSync`,
 *   which reports `EISDIR` as a YAML parse finding.
 * - Anything else — a broken link, a link to a FIFO, a FIFO — is collected
 *   rather than emitted. Emitted, it reached `validateFile`, whose missing-file
 *   guard throws `NOT_FOUND`: written for the `--file` branch, where "no such
 *   file" answers a typo'd argument, and on the walk it aborted the whole run at
 *   the first one. Exit 5, nothing printed about the other forty files, and a
 *   hint naming the command already being run — one `models/x.view.yml` pointing
 *   into a removed worktree was enough.
 *
 * So this emits REGULAR FILES AND LINKS TO THEM, full stop, which is what makes
 * `validateFile`'s two refusals unreachable from here rather than a condition
 * the two functions have to remember to agree on.
 *
 * The directory arm is the one place the two branches see different sets, under
 * two conditions rather than one: a link whose target is INSIDE the root is
 * still reached by its real path — nothing missed, only named differently —
 * UNLESS that real path is under a `SKIP` name, where it is genuinely invisible
 * and no `--file` note applies. A link pointing OUTSIDE the root is not walked,
 * while `--file apps/x.app.yml` reads straight through it. Pinned in
 * `validate.test.ts`.
 */
function walk(root: string, prefix = ""): Walked {
  const out: Walked = { files: [], broken: [] };
  let entries: import("node:fs").Dirent[];
  try {
    entries = readdirSync(join(root, prefix), { withFileTypes: true });
  } catch {
    return out;
  }
  for (const entry of entries) {
    const rel = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isDirectory()) {
      if (SKIP.has(entry.name)) continue;
      const nested = walk(root, rel);
      out.files.push(...nested.files);
      out.broken.push(...nested.broken);
      continue;
    }
    if (entry.name !== APP_MANIFEST) {
      if (extname(entry.name) !== ".yml" && extname(entry.name) !== ".yaml") continue;
      if (!schemaFor(rel)) continue;
    }

    // The only `stat` in this walk, and it runs on links that would otherwise
    // be EMITTED — after `schemaFor`, so an ordinary entry and a link nothing
    // validates both pay nothing. `statSync` FOLLOWS; `lstat` would re-derive
    // what `Dirent` has already said. Its outcomes are the last two arms above:
    // a directory is skipped, and a throw is a link that cannot be followed —
    // for a reason the caller records rather than guesses.
    if (entry.isSymbolicLink()) {
      let target: import("node:fs").Stats;
      try {
        target = statSync(join(root, rel));
      } catch (cause) {
        out.broken.push({ path: rel, code: (cause as NodeJS.ErrnoException).code ?? EUNKNOWN });
        continue;
      }
      if (target.isDirectory()) continue;
      if (!target.isFile()) {
        out.broken.push({ path: rel, code: ENOTFILE });
        continue;
      }
    } else if (!entry.isFile()) {
      // FREE — the `Dirent` already knows, so this costs no syscall. It closes
      // the last door to the whole-run abort: a FIFO, socket or device named
      // `x.app.yml` cleared every arm above, reached `validateFile`, and took
      // its `not a file` throw and the other forty files with it. `walk` now
      // emits regular files and links to them, FULL STOP, which makes that
      // throw unreachable from here — an invariant rather than a divergence
      // the two functions have to remember to agree on.
      out.broken.push({ path: rel, code: ENOTFILE });
      continue;
    }
    out.files.push(rel);
  }
  out.files.sort();
  out.broken.sort((a, b) => a.path.localeCompare(b.path));
  return out;
}

/** Compile every schema once — ajv is slow to compile and fast to run. */
function compileAll(): Map<string, ReturnType<Ajv["compile"]>> {
  const dir = schemasDir();
  const ajv = new Ajv({
    allErrors: true,
    strict: false,
    allowUnionTypes: true,
    // ajv logs every unrecognised `format` to the console, and these schemas
    // are generated by `schemars`, which emits Rust integer widths as formats
    // — so an unconfigured ajv printed twenty "unknown format" lines ahead of
    // the actual result.
    //
    // The known ones are TAUGHT below. What is silenced here is only ajv's
    // `warn`; `error` still comes through, and `warnUnregisteredFormats` then
    // re-raises the one warning that mattered — named, once, instead of once
    // per occurrence — which is the property a bare `logger: false` would have
    // thrown away while a comment claimed otherwise.
    logger: { log: () => {}, warn: () => {}, error: (...a: unknown[]) => console.error(...a) }
  });
  addFormats(ajv);
  registerRustFormats(ajv);

  const compiled = new Map<string, ReturnType<Ajv["compile"]>>();
  // Collected HERE rather than by re-reading the files afterwards: each document
  // is already parsed on this line, and a second pass would need its own
  // try/catch to re-handle a parse error this loop has already reported.
  const formats = new Set<string>();
  for (const [, schema] of SCHEMA_FOR) {
    if (compiled.has(schema)) continue;
    const path = join(dir, schema);
    if (!existsSync(path)) continue;
    try {
      const document = JSON.parse(readFileSync(path, "utf8"));
      formatsInSchemaPosition(document, formats);
      compiled.set(schema, ajv.compile(document));
    } catch (cause) {
      throw new CliError(`could not compile ${schema}`, {
        code: ExitCode.FAILURE,
        detail: (cause as Error).message,
        hint: "the schema is generated by `oxy gen-config-schema` — regenerate it"
      });
    }
  }
  warnUnregisteredFormats(ajv, formats);
  if (compiled.size === 0) {
    // The fix depends on WHERE you are. In a source checkout the schemas are
    // copied in by `prebuild` and gitignored, so "reinstall" is the wrong
    // advice — `pnpm build` is the fix, and telling a developer to reinstall a
    // package they are editing is how a five-second problem becomes an hour.
    //
    // A REMEDY, not a hint: it is the one line here about what to DO, and
    // `CliError.remedy` is what renders that set apart rather than in the run
    // of `→` elaborations. This is also the field's first producer — without
    // one, the error path's half of the split would be wired and untested.
    const inCheckout = existsSync(join(schemasDir(), "..", "..", "..", "json-schemas"));
    throw new CliError("no JSON Schemas are available in this installation", {
      code: ExitCode.FAILURE,
      remedy: inCheckout
        ? "run `pnpm build` in sdk/cli — the schemas are copied from the repo root by prebuild"
        : REINSTALL_REMEDY
    });
  }
  return compiled;
}

/**
 * Every `format` a schema uses in SCHEMA POSITION.
 *
 * Walked rather than regex-scanned over the raw text, because `"format"` is
 * also an ordinary word: a `default:` or an `examples:` holding a serialised
 * config with its own `{"format": "csv"}` is data, not a constraint, and a text
 * scan cannot tell the two apart. A key only counts when its sibling keys mark
 * the object as a schema — `type`, `$ref`, `enum`, `properties` and friends.
 */
export function formatsInSchemaPosition(node: unknown, into: Set<string>): void {
  if (Array.isArray(node)) {
    for (const item of node) formatsInSchemaPosition(item, into);
    return;
  }
  if (!node || typeof node !== "object") return;
  const obj = node as Record<string, unknown>;

  if (typeof obj.format === "string" && SCHEMA_SIBLINGS.some((k) => k in obj)) {
    into.add(obj.format);
  }
  for (const [key, value] of Object.entries(obj)) {
    if (INSTANCE_DATA.has(key)) continue;
    formatsInSchemaPosition(value, into);
  }
}

/**
 * Keywords whose values are INSTANCE DATA, never subschemas.
 *
 * Descending into them is exactly the mistake the text scan made: a `default`
 * holding a serialised config with its own `format` key is a value, not a
 * constraint. `enum` belongs here for the same reason `const` does — it is the
 * plural of `const` — even though it also appears in `SCHEMA_SIBLINGS` below,
 * where it marks the *enclosing* object as a schema.
 *
 * MATCHED ON THE KEY NAME ANYWHERE, so a schema property literally named
 * `default` under `properties:` is skipped too, where these keyword semantics
 * do not apply. Same accepted direction as `SCHEMA_SIBLINGS`: what that costs
 * is a MISSED warning about a format nobody registered, never a false one.
 */
const INSTANCE_DATA = new Set(["default", "examples", "const", "enum"]);

/**
 * A `format` beside any of these is a constraint; alone it is just a word.
 *
 * NOT EXHAUSTIVE, and it does not need to be: `not`, `if`/`then`/`else`,
 * `additionalProperties`, `patternProperties`, `propertyNames` and `contains`
 * all appear in these files and are absent here. A keyword this list misses
 * costs a MISSED warning, never a false one — and every string `format` these
 * schemas emit also carries `type`, which is the first entry.
 */
const SCHEMA_SIBLINGS = [
  "type",
  "$ref",
  "enum",
  "properties",
  "items",
  "allOf",
  "anyOf",
  "oneOf",
  "minimum",
  "maximum",
  "pattern"
];

/**
 * Warn on a `format` that nobody taught ajv.
 *
 * This is the half the silenced logger would otherwise have lost. An
 * unrecognised format is IGNORED by ajv — the constraint silently does not
 * apply — so a `schemars` upgrade that starts emitting a new one would quietly
 * stop validating that field, and with the warning suppressed nothing would
 * say so.
 *
 * WARNS RATHER THAN THROWS, deliberately. What an unknown format costs is one
 * constraint on one field; what a throw here costs is `oxyc validate` for the
 * whole workspace, every file, until someone ships a new release. A Rust type
 * gaining a `u128` is enough to trigger it — `schemars` emits `uint128` — and
 * losing a range check on one integer is not worth losing the command.
 */
function warnUnregisteredFormats(ajv: Ajv, formats: Iterable<string>): void {
  const known = new Set(Object.keys((ajv as unknown as { formats: object }).formats ?? {}));
  const unknown = [...formats].filter((f) => !known.has(f)).sort();
  if (unknown.length === 0) return;
  log.warn(`the schemas use formats ajv does not know: ${unknown.join(", ")}`);
  // Reason and remedy, split for the same reason `SkipReason` splits them —
  // this was one line chaining both across an em dash, one function from the
  // code that stopped doing that.
  log.hint("each is silently ignored, so that one constraint does not apply");
  log.remedy("register them in registerRustFormats()");
}

/**
 * The integer formats `schemars` emits, taught to ajv.
 *
 * They are not JSON Schema formats — they are Rust types. Registering them
 * with their real bounds means `port: 70000` is reported as out of range for a
 * `uint16` instead of passing because ajv did not recognise the word.
 */
function registerRustFormats(ajv: Ajv): void {
  const RANGES: Record<string, [min: number, max: number]> = {
    uint: [0, Number.MAX_SAFE_INTEGER],
    uint8: [0, 255],
    uint16: [0, 65_535],
    uint32: [0, 4_294_967_295],
    uint64: [0, Number.MAX_SAFE_INTEGER],
    int8: [-128, 127],
    int16: [-32_768, 32_767],
    int32: [-2_147_483_648, 2_147_483_647],
    int64: [Number.MIN_SAFE_INTEGER, Number.MAX_SAFE_INTEGER]
  };
  for (const [name, [min, max]] of Object.entries(RANGES)) {
    ajv.addFormat(name, {
      type: "number",
      validate: (value: number) => Number.isInteger(value) && value >= min && value <= max
    });
  }
  // `double` and `float` are emitted for f64/f32 and admit anything numeric.
  for (const name of ["double", "float"]) {
    ajv.addFormat(name, { type: "number", validate: () => true });
  }
}

/** ajv's error objects, flattened to something a person can act on. */
function describe(file: string, errors: ErrorObject[] | null | undefined): Finding[] {
  return (errors ?? []).map((e) => ({
    file,
    // `instancePath` is a JSON pointer; a caller wants a YAML path.
    path: e.instancePath ? e.instancePath.replace(/^\//, "").split("/").join(".") : "(root)",
    message:
      e.keyword === "additionalProperties"
        ? `unknown field \`${(e.params as { additionalProperty?: string }).additionalProperty}\``
        : (e.message ?? "invalid")
  }));
}

/**
 * Validate one file.
 *
 * Returns a SKIP CODE when it could not be checked — distinct from `[]`, which
 * means "checked, and clean". Collapsing the two is how this
 * printed `23 file(s) valid` including every file kind whose schema was
 * missing from the installation: `compileAll` skips a schema that is not on
 * disk, this returned no findings, and the caller counted the file anyway.
 */
function validateFile(
  root: string,
  rel: string,
  compiled: Map<string, ReturnType<Ajv["compile"]>>,
  warnings: Finding[],
  notes: Set<string>
): Finding[] | SkipCode {
  const schema = schemaFor(rel);
  const manifest = isAppManifest(rel);
  // `undefined`, not `[]`, and for the same reason the doc gives: a file with
  // no schema was NOT checked. Unreachable through either door today — `walk`
  // filters on `!schemaFor(rel)` before emitting, and `--file` throws USAGE on
  // the same predicate — but `[]` here is the seed of the exact defect this
  // function's contract exists to prevent, and it grows back silently the day
  // either filter moves.
  if (!schema && !manifest) return KIND_UNKNOWN;
  const validate = schema ? compiled.get(schema) : undefined;
  if (schema && !validate) return SCHEMA_MISSING;

  // NO FILESYSTEM CONDITION MAY ARRIVE LABELLED `(parse)`. Both are settled
  // here, outside the YAML try, because reported from inside it they read as
  // content problems and send the reader looking in a file for something that
  // is the file itself: "ENOENT: no such file or directory" for a path that is
  // not there, "EISDIR: illegal operation on a directory" for a directory named
  // `apps.app.yml` or a link to one. `existsSync` answered only the first —
  // it is true for a directory — so the walk arm got this rule and `--file`
  // kept the exception until now.
  const full = join(root, rel);
  let stats: import("node:fs").Stats;
  try {
    stats = statSync(full);
  } catch (cause) {
    // THE REASON THE STAT GAVE, not the one that is usually true. `no such
    // file` for every failure meant `--file loop.app.yml` on a self-referential
    // link answered that a link `ls` can see does not exist, and `EACCES` sent
    // the reader hunting a path that is present and unreadable — while the walk
    // branch, three hundred lines up, named both correctly. Same helper, so the
    // two branches cannot drift apart again.
    const code = (cause as NodeJS.ErrnoException).code ?? EUNKNOWN;

    // AND `ENOENT` IS TWO CONDITIONS. `statSync` throws it for a path that is
    // not there and for a link whose target is not there, and only the first is
    // "no such file" — `--file stale.app.yml` pointing into a removed worktree
    // was told a link `ls` shows you does not exist, while the walk called the
    // same entry "the target is gone". `lstat` separates them for free.
    //
    // Two names rather than one negated conjunction: `absent` being false for
    // `ELOOP` is about the code, not about anything dangling, and collapsing
    // them made a reader tracing `EACCES` re-derive which conjunct fired.
    const dangling = code === "ENOENT" && isDanglingLink(full);
    const absent = code === "ENOENT" && !dangling;
    const why = whyUnreadable(code);

    throw new CliError(absent ? `no such file: ${rel}` : `cannot read ${rel}`, {
      // THE CODE MUST AGREE WITH THE MESSAGE. 5 means the path does not exist,
      // and an agent branches on the number alone — so returning it for a
      // cycle, a permission error or a dangling link tells a caller the
      // opposite of what it was just told in prose, and the actionable answer
      // ("fix the mode", "repair the link") is one it will never reach.
      code: absent ? ExitCode.NOT_FOUND : ExitCode.FAILURE,
      detail: absent ? undefined : why.reason,
      // ONE RULE FOR BOTH THROWS, now that `CliError` has a `remedy` of its
      // own: the workspace hint always prints, and a remedy — if that arm ever
      // grows one — prints beside it rather than replacing it. The earlier
      // `??` form both displaced the hint and, because `hint` renders through
      // `log.hint`, would have shown the remedy under `→`.
      //
      // No arm of `whyUnreadable` returns a remedy today, so this is dead in
      // exactly the way the deleted `ENOTFILE` fallback was — the difference is
      // that this one costs nothing to be right, where that one was a `??`
      // whose left side could never be taken.
      hint: WHOLE_WORKSPACE_HINT,
      remedy: why.remedy
    });
  }
  if (!stats.isFile()) {
    // Unreachable from the walk, which emits regular files only — this answers
    // a `--file` argument naming something that is not one. FAILURE, not
    // NOT_FOUND: a directory named `x.app.yml` is emphatically there.
    throw new CliError(`not a file: ${rel}`, {
      code: ExitCode.FAILURE,
      detail: stats.isDirectory()
        ? "it is a directory — a name ending .app.yml does not make one a workspace file"
        : whyUnreadable(ENOTFILE).reason,
      // Same rule as the sibling above. `ENOTFILE` is not a private code — a
      // FIFO named `x.app.yml` reaches `listSkipped` through `broken` too — so
      // the day that arm grows a remedy, both printers must show it or they
      // disagree about the same file.
      hint: WHOLE_WORKSPACE_HINT,
      remedy: whyUnreadable(ENOTFILE).remedy
    });
  }

  // The manifest arm, past the same filesystem guards: `validate` is undefined
  // here only when `schema` was, which the guard above allows for `oxy-app.json`.
  if (!validate) return validateAppManifest(root, rel, warnings, notes);

  let parsed: unknown;
  try {
    parsed = parseYaml(readFileSync(full, "utf8"));
  } catch (cause) {
    // A YAML syntax error is the commonest finding of all, and reporting it as
    // a schema violation would send the reader looking at the wrong thing.
    return [
      {
        file: rel,
        path: "(parse)",
        message: (cause as Error).message.split("\n")[0] ?? "unparsable YAML"
      }
    ];
  }
  // An empty document parses to `null`, which every schema rejects with a
  // confusing type error. Say what it actually is.
  if (parsed === null || parsed === undefined) {
    return [{ file: rel, path: "(root)", message: "the file is empty" }];
  }

  return validate(parsed) ? [] : describe(rel, validate.errors);
}

export function runValidate(flags: ValidateFlags, cwd = process.cwd()): void {
  const compiled = compileAll();

  // `cwd` is the base for BOTH paths. It used to be ignored on the `--file`
  // branch, which read `process.cwd()` instead — so the parameter meant two
  // different things depending on a flag.
  const single = flags.file;
  // ONE SPELLING FOR BOTH BRANCHES, so `root` means one thing regardless of the
  // flag. `findWorkspace` compares physical paths and returns one; rooting the
  // `--file` branch at the raw `cwd` left the two disagreeing about how to name
  // the same directory. No output differs from that today — `relative()` is
  // computed against whichever base was used, and each branch agrees with
  // itself — so it is an invariant, not a fix: it keeps the next thing to print
  // `root` from printing two forms.
  const base = physical(resolve(cwd));
  const root = single ? base : findWorkspace(cwd);

  // THE ARGUMENT IS NOT RESOLVED, and that is the whole point of the line.
  // `schemaFor` decides a file's KIND from its basename, so following a link
  // here changes what the file is: `ln -s tpl/base.yml my.app.yml` becomes
  // `base.yml`, matches nothing, and `--file my.app.yml` exits USAGE on a file
  // the whole-workspace walk validates happily — `walk` reads directories with
  // `withFileTypes`, and a `Dirent` does not follow links, so it sees the link's
  // own name. In an Oxy workspace the kind of a file IS its name, which makes
  // the walk the correct one and this the branch that had to change back.
  // `base` already carries every bit of canonicalisation this needed.
  const walked: Walked = single
    ? { files: [relative(root, resolve(base, single))], broken: [] }
    : walk(root);
  const { files, broken } = walked;

  if (single && !schemaFor(files[0] ?? "") && !isAppManifest(files[0] ?? "")) {
    throw new CliError(`oxyc validate does not know how to check ${single}`, {
      code: ExitCode.USAGE,
      hint: `it checks ${[...SCHEMA_FOR.map(([s]) => s), APP_MANIFEST].join(", ")}`
    });
  }
  if (files.length === 0 && broken.length === 0) {
    // STILL A DOCUMENT ON STDOUT. `--json` promises one, and an empty stdout
    // makes `oxyc validate --json | jq` fail on the one workspace shape where
    // the answer is simply "nothing to check".
    if (flags.json) {
      const report: ValidateReport = { checked: 0, unchecked: [], broken: [], findings: [] };
      process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
      return;
    }
    log.warn(`no validatable YAML found under ${root}`);
    log.hint("oxyc validate --file <path>   to check one file");
    return;
  }

  const findings: Finding[] = [];
  const warnings: Finding[] = [];
  const notes = new Set<string>();
  const checked: string[] = [];
  const unchecked: SkippedFile<SkipCode>[] = [];
  for (const rel of files) {
    const result = validateFile(root, rel, compiled, warnings, notes);
    if (typeof result === "string") {
      unchecked.push({ path: rel, code: result });
      continue;
    }
    checked.push(rel);
    findings.push(...result);
  }

  // NOTHING READ, BUT SOMETHING NAMED. The commit that changed the word left the
  // number saying `valid`: with every file in `unchecked` or `broken`, stderr
  // said "N file(s) NOT checked" and the process exited 0. On `--file` there is
  // no partial answer to defend — the caller named one file, it was never
  // opened, and the command answered success. `util/errors.ts` exists so that
  // "printed an error and exited 0" is unrepresentable; this reached it through
  // the verdict rather than through an error.
  //
  // A genuinely EMPTY workspace still exits 0 — that is the case "nothing to
  // check" honestly describes, and THE EARLY RETURN ABOVE is what protects it,
  // not the second conjunct here. `checked === 0` on this line already implies
  // something was named: the only ways to get here with nothing checked are
  // every file landing in `unchecked`, or no files and a non-empty `broken`.
  // The conjunct is kept for the arithmetic in the message — "all 0 candidate
  // file(s) were skipped" would be nonsense — and is unreachable today, which
  // is said here so it is not mistaken for the discriminator. A mutation
  // removing it does not fail the suite, and no test claims otherwise.
  const nothingRead = checked.length === 0 && unchecked.length + broken.length > 0;

  if (flags.json) {
    reportWarnings(warnings);
    reportNotes(notes);
    const report: ValidateReport = { checked: checked.length, unchecked, broken, findings };
    process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
    if (findings.length > 0) throw silentFailure(findings.length, checked.length);
    if (nothingRead) throw nothingReadFailure(unchecked.length + broken.length);
    return;
  }

  // NAMED, and named BEFORE the verdict. A file that was not read must never be
  // counted as valid — that is the answer a caller acts on. Two reasons a file
  // gets here, kept apart because the fix differs: no schema for its kind in
  // this installation, or a link that could not be followed — with the reason
  // that link actually gave, not one assumed for it.
  // A COUNT ON THE LINE, A REASON WHERE IT BELONGS. The count used to carry the
  // reason, which worked while `unchecked` had one producer and asserted the
  // wrong remedy the moment it had two: "reinstall" is right for a schema
  // absent from the installation and useless for a kind `oxyc` does not map at
  // all. Per-file was the first correction and repeated one long sentence ten
  // times; `listSkipped` settles where it goes.
  //
  // The DESCRIPTORS here are the mixed-bucket wording. `unchecked`'s is
  // unreachable today — `KIND_UNKNOWN` cannot be produced by a run, so that
  // bucket always agrees and always hoists — and is written for the day it can.
  listSkipped("no schema was applied", unchecked, whyUnchecked);
  // "symlink(s)" was right when this bucket held only links; it now holds a
  // FIFO or device named like a workspace file too, which is not one.
  listSkipped("could not read them", broken, whyUnreadable);
  reportWarnings(warnings);
  reportNotes(notes);

  if (findings.length === 0) {
    // `0 file(s) valid` in green was the line a skimmer read when nothing had
    // been opened at all. The earlier fix changed the word to `nothing
    // checked`; making the EXIT CODE agree removes the case entirely, because
    // every way to reach zero checked files now either returns above (a
    // workspace with nothing in it) or throws here. So the branch that printed
    // it is gone rather than left reading as live — `checked.length` is
    // guaranteed non-zero on this line.
    if (nothingRead) throw nothingReadFailure(unchecked.length + broken.length);
    process.stdout.write(`${out.green(`${checked.length} file(s) valid`)}\n`);
    return;
  }

  process.stdout.write(
    `${heading(`${findings.length} problem(s) in ${checked.length} file(s)`)}\n`
  );
  process.stdout.write(
    `${table(findings, [
      { header: "FILE", value: (f) => f.file },
      { header: "AT", value: (f) => f.path },
      { header: "PROBLEM", value: (f) => f.message }
    ])}\n`
  );
  log.info("structural checks only — `oxy validate` also resolves databases: and llm.ref");
  throw silentFailure(findings.length, checked.length);
}

/**
 * An `oxy-app.json`: where its data goes, and what its functions call — not
 * its schema.
 *
 * Errors come back as findings and fail the run like a schema violation;
 * warnings are collected for `reportWarnings` and do not. Unparsable JSON is a
 * `(parse)` finding, the YAML arm's rule — not the throw `readAppManifest`
 * makes, which would end the walk at the first broken manifest.
 *
 * The function lint runs beside the placement check on the same manifest. Its
 * offline half — capabilities, isolate globals, the `destinations` allowlist —
 * is answered here; the half that needs the database's engine is `publish`'s,
 * and `notes` says so once, rather than reporting a clean run as complete.
 */
function validateAppManifest(
  root: string,
  rel: string,
  warnings: Finding[],
  notes: Set<string>
): Finding[] {
  const full = join(root, rel);
  let parsed: unknown;
  try {
    parsed = JSON.parse(readFileSync(full, "utf8"));
  } catch (cause) {
    return [{ file: rel, path: "(parse)", message: (cause as Error).message.split("\n")[0] ?? "" }];
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    return [{ file: rel, path: "(root)", message: "the top level must be an object" }];
  }
  const manifest = parsed as Record<string, unknown>;
  const appDir = dirname(full);
  // `publish`'s order, minus the flags and env it has and this does not.
  const slug =
    typeof manifest.slug === "string" && manifest.slug.trim()
      ? manifest.slug
      : inferOrgApp(appDir).app;

  const prefix = dirname(rel) === "." ? "" : `${dirname(rel)}/`;
  const findings: Finding[] = [];
  for (const issue of checkAppPlacement(appDir, manifest, slug)) {
    const finding = { file: `${prefix}${issue.file}`, path: issue.path, message: issue.message };
    (issue.level === "error" ? findings : warnings).push(finding);
  }

  const lint = lintAppFunctions(appDir, manifest);
  for (const issue of lint.issues) {
    findings.push({
      file: `${prefix}${issue.file}`,
      path: issue.path,
      message: `[function-lint/${issue.rule}] ${issue.message}`
    });
  }
  for (const line of lint.skipped) notes.add(`${prefix}${APP_MANIFEST}: ${line}`);
  if (lint.writes.length > 0) {
    notes.add(
      `${prefix}${APP_MANIFEST}: the engine half of the function lint (\`upsert\` and ` +
        "`ctx.tx` dialects, customer-warehouse writes) needs the server and is skipped offline — " +
        "`oxyc publish` runs it before the upload"
    );
  }
  return findings;
}

/**
 * Warnings from the manifest checks: printed, never counted as problems.
 *
 * On stderr in both modes, the channel every other warning here takes, so
 * `--json` keeps one document on stdout and its shape does not move.
 */
function reportWarnings(warnings: Finding[]): void {
  for (const w of warnings) log.warn(`${w.file} (${w.path}): ${w.message}`);
}

/**
 * What the function lint did NOT do, once each.
 *
 * `info`, not `warn`: a skipped engine check is the ordinary offline case, not
 * a degraded run — but it is said, because a clean `validate` on a function
 * that upserts into ClickHouse is not a clean publish.
 */
function reportNotes(notes: Set<string>): void {
  for (const note of notes) log.info(note);
}

/**
 * Name each skipped file with its reason, capped, and say when the cap bit.
 *
 * ONE SHARED REASON IS PRINTED ONCE. Moving the reason per file fixed a message
 * that asserted one producer's remedy for both — but a workspace where every
 * file was skipped for the same cause then repeated a sentence ten times, which
 * is worse to read than the wrong-remedy line was. So the reason goes on the
 * count when the bucket agrees and beside each path when it does not, which is
 * the only case that needed per-file wording in the first place.
 *
 * The trailing count matters for the same reason: `12 file(s) NOT checked` over
 * ten paths reads as a display bug rather than a cap.
 */
function listSkipped<C extends string>(
  descriptor: string,
  files: SkippedFile<C>[],
  why: (code: C) => SkipReason
): void {
  // The guard lives beside the print. It was at both call sites while the
  // `log.warn` was too; splitting them left `0 file(s) NOT checked` reachable
  // from any third caller, two functions away from the check preventing it.
  if (files.length === 0) return;

  const codes = [...new Set(files.map((f) => f.code))];
  const shared = codes.length === 1 ? why(codes[0] as C) : undefined;

  // ONE RULE FOR BOTH BUCKETS: a shared reason REPLACES the category
  // descriptor rather than chaining after it. `1 file(s) NOT checked — could
  // not read them — the target is gone` was two clauses saying one thing, and
  // the other call site had dropped its descriptor to avoid exactly that — so
  // the two headlines were being built to different rules.
  log.warn(`${files.length} file(s) NOT checked — ${shared?.reason ?? descriptor}`);

  const SHOWN = 10;
  for (const file of files.slice(0, SHOWN)) {
    log.hint(shared ? file.path : `${file.path} — ${why(file.code).reason}`);
  }
  if (files.length > SHOWN) log.hint(`… and ${files.length - SHOWN} more`);

  // REMEDIES LAST, DEDUPED, AND NEVER CHAINED ONTO THE REASON. The one
  // reachable shared headline used to carry both, at 140 characters — and a
  // remedy repeated per file is the noise that made the reason shared in the
  // first place. One line per distinct remedy, after the paths it applies to,
  // through `log.remedy` rather than `log.hint`: `→` elaborates the line above
  // and so reads in sequence with the paths, while a remedy is the one line
  // about what to DO and wants the blank lines that say so.
  //
  // ONE CALL, not a loop: `log.remedy` brackets its whole argument list, so N
  // remedies through N calls would be N blocks with doubled blanks between
  // them. Spreading a possibly-empty array is also what makes that function's
  // emptiness guard reachable rather than protection-shaped.
  const remedies = [...new Set(codes.map((c) => why(c).remedy))].filter((r) => r !== undefined);
  log.remedy(...remedies);
}

/**
 * Nothing was read, and it was not because there was nothing to read.
 *
 * FAILURE (1), for the same reason `silentFailure` is: the command ran and the
 * answer is bad. The files themselves are already named above — this only
 * supplies the number an agent branches on, which is the half that was saying
 * success.
 *
 * NOT `UNAVAILABLE` (7), though the nearest precedent on this branch — `launch`
 * reporting a missing `claude` — chose 7 on the "correct invocation, incomplete
 * environment" framing, and a schema missing from the installation fits that
 * exactly. This throw also covers a workspace whose only candidates were
 * unreadable links, which is a workspace problem and not an install one, so the
 * code covering both has to be the catch-all. Written here because the two
 * commands otherwise answer a same-looking shape with different numbers and
 * nothing in the tree says why.
 */
function nothingReadFailure(named: number): CliError {
  return new CliError(`nothing was checked — all ${named} candidate file(s) were skipped`, {
    code: ExitCode.FAILURE
  });
}

/**
 * The findings are already printed, so this carries the exit code and nothing
 * else — a second copy of the list on stderr would just be noise.
 */
function silentFailure(findings: number, files: number): CliError {
  // FAILURE (1), not REQUEST (6). The contract documents 6 as "the server
  // answered 4xx" and this command never makes a request — an agent branching
  // on 6 would conclude its own call was malformed rather than that the
  // workspace has findings. Same mismatch as `launch` reporting a missing
  // `claude` as USAGE, fixed in an earlier round.
  return new CliError(`${findings} problem(s) across ${files} file(s)`, {
    code: ExitCode.FAILURE
  });
}

/**
 * The workspace root: the nearest ancestor with a `config.yml`, else `cwd`.
 *
 * Walks UP rather than assuming the cwd, because `oxyc validate` is most
 * useful from wherever you happen to be inside a workspace — and a customer
 * repo keeps its workspace in a subdirectory as often as not.
 */
function findWorkspace(from: string): string {
  // RESOLVED THROUGH SYMLINKS, both sides, or the home stop silently does not
  // fire. `os.homedir()` returns `$HOME` verbatim while `process.cwd()` is
  // `getcwd(3)` and therefore physical — so on a machine whose home or temp
  // directory crosses a link (macOS `$TMPDIR` is under `/var`, itself a link to
  // `/private/var`) the two spellings of one directory never compare equal and
  // the walk climbs straight past it. `git rev-parse --show-toplevel` is
  // physical already, which is why the repo stop was never affected.
  const start = physical(resolve(from));
  const home = physical(resolve(homedir()));
  const fsRoot = parse(start).root;
  // Through `physical` as well, though `git rev-parse --show-toplevel` resolves
  // via getcwd and is already physical for the ordinary case. It costs one call
  // and makes "every path compared here is physical" true by construction
  // rather than by an argument about another program's output — and the failure
  // mode if that argument is ever wrong is the silent-inert one above.
  const gitRoot = repoRoot(start);
  const repo = gitRoot === undefined ? undefined : physical(gitRoot);

  let dir = start;
  for (;;) {
    // `~` AND `/` ARE NEVER WORKSPACE ROOTS. A `config.yml` at either exists by
    // accident, and adopting one makes the walk below it a whole home directory
    // or a whole filesystem — the original symptom. A git root holding one IS a
    // workspace, so only these two are excluded.
    //
    // This is the half that does the work for a cwd outside home and outside a
    // repo: the climb from `/tmp/x/sub` is not shortened by any stop, so `/`
    // is reached and only this exclusion keeps a stray `/config.yml` from
    // becoming the root. It does NOT protect `/tmp/config.yml`, which is a real
    // ancestor and therefore a plausible workspace — see the note below.
    //
    // Running `oxyc validate` FROM `~` still walks the whole home directory:
    // the exclusion declines to adopt it, the home stop returns `start`, and
    // `start` is `~`. That is asking to validate here, which is a different
    // thing from climbing into it by accident.
    if (dir !== home && dir !== fsRoot && existsSync(join(dir, "config.yml"))) return dir;

    // Two stops, both observable. The git root is the only real evidence of
    // where a project ends; home bounds a cwd inside it so the walk cannot
    // cross into `/Users` or `/home` and adopt a stray file there. The
    // filesystem root is NOT among them on purpose — `parent === dir` below
    // already returns there, and listing it would be a term that changes
    // nothing while reading as protection.
    if (dir === repo || dir === home) return start;

    const parent = resolve(dir, "..");
    if (parent === dir) return start;
    dir = parent;
  }
}

/**
 * Is this a link whose target is missing?
 *
 * `statSync` gives `ENOENT` for both an absent path and a dangling link, and
 * only the first is "no such file". `lstat` does not follow, so it succeeds on
 * exactly the second.
 */
function isDanglingLink(path: string): boolean {
  try {
    return lstatSync(path).isSymbolicLink();
  } catch {
    return false;
  }
}

/**
 * The physical path, or the input when it cannot be resolved.
 *
 * `realpathSync` throws on a path that does not exist, so the fallback is the
 * ordinary case for a `--file` argument naming a typo. Where it matters is
 * `$HOME`: pointed at something unmounted, the raw logical spelling comes back
 * and the HOME STOP goes quiet again — harmless, since there is nothing to walk
 * into under a home that is not there, but worth knowing which guard it is.
 */
function physical(path: string): string {
  try {
    return realpathSync(path);
  } catch {
    return path;
  }
}

/*
 * NO LEVEL BOUND, deliberately. It would have to be small enough to stop a walk
 * out of `/tmp/x/sub` — two levels — and anything that small refuses a workspace
 * you are legitimately deep inside. Finding a `config.yml` in a real ancestor is
 * the feature; the exclusion above is what keeps "ancestor" from meaning `~`
 * or `/`.
 */

/** Exposed for the tests: is this path a directory we would walk into? */
export function walkable(name: string): boolean {
  return !SKIP.has(name);
}

/** Exposed for the tests. */
export function listValidatable(root: string): string[] {
  return statSync(root).isDirectory() ? walk(root).files : [];
}
