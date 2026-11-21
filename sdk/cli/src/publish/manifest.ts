/**
 * `oxy-app.json` as publishing reads it.
 *
 * ONE CONTRACT. The shape is `OxyAppManifest` from `@oxy-hq/sdk/manifest` —
 * the type the JSON Schema is generated from — and `loadPublishManifest` runs
 * the SDK's validator over the file before anything is built, so a publish
 * refuses exactly what `oxyc validate` and the Vite plugin refuse, in the same
 * words. The name rules (`isValidSlug`, `isValidFunctionName`) come from the
 * same module; this file used to carry its own copies of both.
 *
 * `context/target.ts` owns identity and `environments` and reads the file
 * leniently, because a command told `--target` must not fail on a manifest it
 * does not need. This is the strict read.
 */

import { join } from "node:path";
import {
  type ManifestIssue,
  type OxyAppFunctionManifest,
  type OxyAppManifest,
  validateOxyAppManifest
} from "@oxy-hq/sdk/manifest";
import { readAppManifest } from "../context/target.js";
import * as log from "../ui/log.js";
import { CliError, ExitCode } from "../util/errors.js";

/** The manifest a publish reads: the SDK's type, after validation. */
export type PublishManifest = OxyAppManifest;

/** One `functions.<name>` entry. */
export type FunctionSpec = OxyAppFunctionManifest;

/**
 * Longest writer name: Postgres caps identifiers at 63 bytes, and the longest
 * derived form is `app_<name>_rw` — the server's OLTP `MAX_NAME_LEN`.
 */
export const WRITER_NAME_MAX = 63 - "app_".length - "_rw".length;

/** `validate_name`, as a sentence to put in an error. */
export const WRITER_NAME_SHAPE = `1-${WRITER_NAME_MAX} characters: a lowercase letter, then lowercase letters, digits or underscores`;

/** `app_writer_name`: how a slug becomes a writer, as a sentence. */
export const WRITER_NAME_RULE = `\`-\` becomes \`_\`, and the result must be ${WRITER_NAME_SHAPE}`;

/** The server's OLTP `validate_name`: the same rule, checked before upload. */
export function isValidWriterName(name: string): boolean {
  return new RegExp(`^[a-z][a-z0-9_]{0,${WRITER_NAME_MAX - 1}}$`).test(name);
}

/**
 * The writer an app owns, DERIVED from its slug — the server's OLTP
 * `app_writer_name`. `undefined` when the slug cannot back a schema.
 *
 * A slug that already contains `_` is refused rather than normalised: `my_app`
 * would alias the hyphenated sibling `my-app` onto one schema and role.
 */
export function appWriterName(slug: string): string | undefined {
  if (slug.includes("_")) return undefined;
  const name = slug.replaceAll("-", "_");
  return isValidWriterName(name) ? name : undefined;
}

/**
 * `<dir>/oxy-app.json`, validated, or `undefined` when there is none.
 *
 * A broken one throws: publishing past it used to ship a bundle with no
 * functions and an identity taken from flags or the directory. An ERROR from
 * the schema — a wrong type, a bad name, a missing required field, a
 * notification catalogue the server refuses — throws too, listing every one,
 * because the server would refuse the upload and a build is minutes.
 * WARNINGS (a field the schema does not know, a catalogue kind nothing sends)
 * are printed and the publish goes on: the platform ignores what it does not
 * read, and refusing a field a newer SDK added would break every app on a
 * slightly older `oxyc`.
 */
export function loadPublishManifest(
  dir: string,
  warn: (line: string) => void = log.warn
): PublishManifest | undefined {
  const raw = readAppManifest(dir);
  if (raw === undefined) return undefined;
  const issues = validateOxyAppManifest(raw);
  const errors = issues.filter((issue) => issue.level === "error");
  if (errors.length > 0) throw invalidManifest(join(dir, "oxy-app.json"), errors);
  for (const issue of issues) {
    if (issue.level === "warning") warn(`oxy-app.json (${issue.path}): ${issue.message}`);
  }
  return raw as unknown as PublishManifest;
}

function invalidManifest(path: string, errors: ManifestIssue[]): CliError {
  return new CliError(`${path} is not a valid oxy-app.json: ${errors.length} problem(s)`, {
    code: ExitCode.USAGE,
    detail: errors.map((issue) => `  ${issue.path}: ${issue.message}`).join("\n"),
    remedy: "fix each in oxy-app.json — `oxyc validate` lists them without building"
  });
}

/** A blank manifest value is no value: `"install": ""` falls back to the default. */
function orDefault(value: string | undefined, fallback: string): string {
  return value?.trim() ? value : fallback;
}

export interface BuildSteps {
  install: string;
  command: string;
  outDir: string;
}

/**
 * Install / build / output directory. `out` matches the directory the Vite
 * plugin forces, so an identity-only manifest publishes without a `build` block.
 */
export function buildSteps(manifest: PublishManifest | undefined): BuildSteps {
  return {
    install: orDefault(manifest?.build?.install, "pnpm install"),
    command: orDefault(manifest?.build?.command, "pnpm build"),
    outDir: orDefault(manifest?.build?.outDir, "out")
  };
}

/** Declared function names, or an empty list. */
export function declaredFunctions(manifest: PublishManifest | undefined): string[] {
  const functions = manifest?.functions;
  return functions && typeof functions === "object" ? Object.keys(functions) : [];
}

const isObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/**
 * Source entry relative to the app directory. Default `functions/<name>.ts`.
 *
 * Takes the manifest as a file holds it too: the lints read one before it is
 * validated, and a wrong `entry` is theirs to report, not a reason to crash.
 */
export function functionEntry(
  manifest: PublishManifest | Record<string, unknown> | undefined,
  name: string
): string {
  const functions = manifest?.functions;
  const spec = isObject(functions) ? functions[name] : undefined;
  const entry = isObject(spec) && typeof spec.entry === "string" ? spec.entry : undefined;
  return orDefault(entry, `functions/${name}.ts`);
}
