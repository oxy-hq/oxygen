/**
 * `oxyc migrate-automations` — move a workspace from the legacy Procedures /
 * Workflows naming to the canonical **Automations** naming.
 *
 * Ported from the Rust `oxy migrate-automations` (removed after one release of a
 * stub that points here). A pure local file rewrite: no network, no token.
 *
 * Rooted at the given directory, it:
 *   1. renames every `*.procedure.yml` / `*.workflow.yml` to `*.automation.yml`;
 *   2. rewrites references to the old extensions in every `.yml` / `.yaml` /
 *      `.sql` file, which covers exact path refs (`src:`, `workflow_ref:`) and
 *      glob includes (`procedures/*.procedure.yml`) alike, because it is a plain
 *      substring replacement.
 *
 * The platform still accepts `.procedure.yml` (and no longer recognises
 * `.workflow.yml` at all), so this is opt-in and safe to run incrementally.
 * `--dry-run` prints the rename plan and touches nothing.
 *
 * NEVER OVERWRITES. A source whose target already exists, or two sources that
 * map to one target (`x.procedure.yml` beside `x.workflow.yml`), is a collision:
 * reported, skipped, and left for a person.
 *
 * KEPT FROM THE RUST ON PURPOSE, so a run here does what a run there did:
 * - only the `.yml` spellings are renamed, never `.procedure.yaml`;
 * - references are rewritten only when at least one file was renamed, and
 *   never on a dry run;
 * - the rewrite is blind, so a reference to a COLLIDED file is rewritten too
 *   and points at a name that does not exist until the collision is resolved;
 * - collisions alone are not a failure: the run exits 0.
 */

import {
  readdirSync,
  readFileSync,
  realpathSync,
  renameSync,
  statSync,
  writeFileSync
} from "node:fs";
import { basename, dirname, join, relative, resolve, sep } from "node:path";
import * as log from "../ui/log.js";
import { out } from "../ui/tty.js";
import { CliError, ExitCode } from "../util/errors.js";

/**
 * Directories never walked — hidden and build dirs that may hold stray copies.
 * Any other dot-prefixed directory is skipped too (see `collectFiles`).
 *
 * The Rust list exactly, NOT `validate.ts`'s superset: a workspace keeping a
 * `.procedure.yml` under `out/` had it renamed by the Rust, and still does.
 */
export const SKIP_DIRS: ReadonlySet<string> = new Set([
  ".git",
  ".worktrees",
  ".oxy_state",
  "target",
  "node_modules",
  "dist",
  "build",
  ".venv"
]);

/** Legacy extensions that become the canonical one. */
export const OLD_EXTENSIONS = [".procedure.yml", ".workflow.yml"] as const;
export const NEW_EXTENSION = ".automation.yml";

/** File kinds whose textual references are rewritten. */
const REWRITE_EXTENSIONS = [".yml", ".yaml", ".sql"] as const;

export interface MigrateAutomationsFlags {
  /** Workspace directory, relative to `cwd`. Defaults to `.`. */
  path?: string;
  dryRun?: boolean;
}

export interface Rename {
  from: string;
  to: string;
}

/** What a run did (or, on a dry run, would do). Paths are absolute. */
export interface MigrateReport {
  renames: Rename[];
  collisions: string[];
  /** Files whose references were rewritten. Always empty on a dry run. */
  rewritten: string[];
}

/** Where the run's lines go. Injected so tests can pin the transcript. */
export interface MigrateIo {
  /** The answer: the plan, each rename, the summary. */
  out(line: string): void;
  /** A warning about a file left alone. */
  warn(line: string): void;
  /** A line elaborating the warning above it. */
  hint(line: string): void;
}

const defaultIo: MigrateIo = {
  out: (line) => process.stdout.write(`${line}\n`),
  warn: log.warn,
  hint: log.hint
};

/** The `.automation.yml` target for a legacy file, or `undefined` if not legacy. */
export function targetPath(path: string): string | undefined {
  const name = basename(path);
  for (const ext of OLD_EXTENSIONS) {
    if (name.endsWith(ext)) {
      return join(dirname(path), `${name.slice(0, -ext.length)}${NEW_EXTENSION}`);
    }
  }
  return undefined;
}

/**
 * Every file under `dir`, sorted, skipping `SKIP_DIRS` and dot-prefixed dirs.
 *
 * A symlink to a directory is followed, as the Rust's `is_dir()` did; a broken
 * link is a file. A directory reached twice through links is walked once, where
 * the Rust recursed until its stack ran out.
 */
export function collectFiles(dir: string, seen = new Set<string>()): string[] {
  const real = realpathSync(dir);
  if (seen.has(real)) return [];
  seen.add(real);
  const files: string[] = [];
  const names = readdirSync(dir).sort();
  for (const name of names) {
    const path = join(dir, name);
    if (isDirectory(path)) {
      if (SKIP_DIRS.has(name) || name.startsWith(".")) continue;
      files.push(...collectFiles(path, seen));
    } else {
      files.push(path);
    }
  }
  return files;
}

function isDirectory(path: string): boolean {
  try {
    return statSync(path).isDirectory();
  } catch {
    return false; // a broken link
  }
}

/**
 * The rename plan, and the sources skipped as collisions.
 *
 * A source collides when its target is already among `files`, or when more than
 * one source maps to the same target. Both sources of a shared target are
 * skipped: picking one would be a guess.
 */
export function planRenames(files: readonly string[]): {
  renames: Rename[];
  collisions: string[];
} {
  const existing = new Set(files);
  const sources = files.flatMap((from) => {
    const to = targetPath(from);
    return to === undefined ? [] : [{ from, to }];
  });
  const claimed = new Set<string>();
  const duplicated = new Set<string>();
  for (const { to } of sources) {
    if (claimed.has(to)) duplicated.add(to);
    claimed.add(to);
  }
  const renames: Rename[] = [];
  const collisions: string[] = [];
  for (const source of sources) {
    if (duplicated.has(source.to) || existing.has(source.to)) collisions.push(source.from);
    else renames.push(source);
  }
  return { renames, collisions };
}

/** Replace every legacy extension in `text` with the canonical one. */
export function rewriteText(text: string): string {
  let updated = text;
  for (const ext of OLD_EXTENSIONS) updated = updated.replaceAll(ext, NEW_EXTENSION);
  return updated;
}

const utf8 = new TextDecoder("utf-8", { fatal: true });

/**
 * Rewrite legacy references in every `.yml` / `.yaml` / `.sql` file under
 * `root`. Returns the files actually changed.
 *
 * A file that is not valid UTF-8 is skipped, as the Rust skipped it: decoding
 * leniently would write U+FFFD over every byte it could not read.
 */
export function rewriteReferences(root: string, io: MigrateIo): string[] {
  const changed: string[] = [];
  for (const path of collectFiles(root)) {
    if (!REWRITE_EXTENSIONS.some((ext) => basename(path).endsWith(ext))) continue;
    let original: string;
    try {
      original = utf8.decode(readFileSync(path));
    } catch {
      continue;
    }
    const updated = rewriteText(original);
    if (updated === original) continue;
    try {
      writeFileSync(path, updated);
    } catch (cause) {
      throw new CliError(`could not write ${rel(root, path)}: ${(cause as Error).message}`, {
        code: ExitCode.FAILURE
      });
    }
    io.out(`updated refs in ${rel(root, path)}`);
    changed.push(path);
  }
  return changed;
}

/** Entry point for `oxyc migrate-automations`. */
export function runMigrateAutomations(
  flags: MigrateAutomationsFlags,
  cwd = process.cwd(),
  io: MigrateIo = defaultIo
): MigrateReport {
  const given = flags.path ?? ".";
  const root = resolve(cwd, given);
  requireDirectory(root, given);

  const { renames, collisions } = planRenames(collectFiles(root));
  reportCollisions(root, collisions, io);

  if (renames.length === 0) {
    if (collisions.length === 0) {
      io.out("No .procedure.yml / .workflow.yml files found — nothing to migrate.");
    }
    return { renames, collisions, rewritten: [] };
  }

  if (flags.dryRun) {
    io.out("Planned renames:");
    for (const r of renames) io.out(`  ${rel(root, r.from)} -> ${rel(root, r.to)}`);
    io.out("");
    io.out(
      `Dry run: ${renames.length} file(s) would be renamed. Re-run without --dry-run to apply.`
    );
    return { renames, collisions, rewritten: [] };
  }

  applyRenames(root, renames, io);
  const rewritten = rewriteReferences(root, io);
  io.out("");
  io.out(
    out.green(
      `✓ Renamed ${renames.length} file(s); updated references in ${rewritten.length} file(s).`
    )
  );
  if (collisions.length > 0) {
    io.warn(
      `skipped ${collisions.length} file(s) due to naming collisions (listed above) — resolve them by hand.`
    );
  }
  return { renames, collisions, rewritten };
}

/** A missing root is NOT_FOUND, as `oxyc validate` treats a missing file; a non-directory is USAGE. */
function requireDirectory(root: string, given: string): void {
  let isDir: boolean;
  try {
    isDir = statSync(root).isDirectory();
  } catch {
    throw new CliError(`${given} does not exist`, {
      code: ExitCode.NOT_FOUND,
      hint: "pass the workspace directory, or run from inside it"
    });
  }
  if (!isDir) {
    throw new CliError(`${given} is not a directory`, {
      code: ExitCode.USAGE,
      hint: "pass the workspace directory, not a file in it"
    });
  }
}

function applyRenames(root: string, renames: readonly Rename[], io: MigrateIo): void {
  for (const r of renames) {
    try {
      renameSync(r.from, r.to);
    } catch (cause) {
      throw new CliError(
        `could not rename ${rel(root, r.from)} -> ${rel(root, r.to)}: ${(cause as Error).message}`,
        { code: ExitCode.FAILURE }
      );
    }
    io.out(`renamed ${rel(root, r.from)} -> ${rel(root, r.to)}`);
  }
}

function reportCollisions(root: string, collisions: readonly string[], io: MigrateIo): void {
  if (collisions.length === 0) return;
  io.warn("naming collisions (skipped — target already exists or two files map to one name):");
  for (const c of collisions) io.hint(rel(root, c));
}

/** `root`-relative, with `/` on every platform so the transcript is one shape. */
function rel(root: string, path: string): string {
  return relative(root, path).split(sep).join("/");
}
