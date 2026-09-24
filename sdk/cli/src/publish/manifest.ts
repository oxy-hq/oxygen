/**
 * The half of `oxy-app.json` that publishing reads, and the two name rules the
 * server enforces on it.
 *
 * `context/target.ts` owns identity and `environments`; this adds `build` and
 * `functions`. Both read the same file and ignore what they do not know, so an
 * SDK field added tomorrow cannot break a publish today.
 */

import { readAppManifest } from "../context/target.js";

export interface FunctionSpec {
  entry?: string;
  /** Databases `ctx.warehouse.*` writes may target. Absent → no writes. */
  destinations?: string[];
  /** Gate for `ctx.airhouse`, which writes the app's own `app_<writer>` schema. */
  airhouse?: { enabled?: boolean };
  /**
   * `<database>: <reason>` — the explicit exception that lets this function
   * write a CUSTOMER warehouse named in `destinations`. Read-only otherwise.
   */
  customerWarehouseWrites?: Record<string, string>;
  secrets?: { write?: boolean };
  /**
   * A function a release gate may run to verify what it just shipped.
   * `oxyc checks run` runs exactly these, and it is the only function a
   * machine credential is allowed to trigger.
   */
  check?: boolean;
}

export interface PublishManifest {
  slug?: string;
  orgSlug?: string;
  build?: { install?: string; command?: string; outDir?: string };
  functions?: Record<string, FunctionSpec>;
  /** OLTP (Postgres) migrations, run on promote. A directory inside the bundle. */
  migrations?: { dir?: string };
  /** Airhouse (DuckLake) migrations for `app_<writer>`, run once each on promote. */
  airhouseMigrations?: { dir?: string };
}

/**
 * Longest writer name: Postgres caps identifiers at 63 bytes, and the longest
 * derived form is `app_<name>_rw` — `crates/oltp/src/schema.rs` `MAX_NAME_LEN`.
 */
export const WRITER_NAME_MAX = 63 - "app_".length - "_rw".length;

/** `validate_name`, as a sentence to put in an error. */
export const WRITER_NAME_SHAPE = `1-${WRITER_NAME_MAX} characters: a lowercase letter, then lowercase letters, digits or underscores`;

/** `app_writer_name`: how a slug becomes a writer, as a sentence. */
export const WRITER_NAME_RULE = `\`-\` becomes \`_\`, and the result must be ${WRITER_NAME_SHAPE}`;

/** `crates/oltp/src/schema.rs` `validate_name`. */
export function isValidWriterName(name: string): boolean {
  return new RegExp(`^[a-z][a-z0-9_]{0,${WRITER_NAME_MAX - 1}}$`).test(name);
}

/**
 * The writer an app owns, DERIVED from its slug — `app_writer_name` in
 * `crates/oltp/src/schema.rs`. `undefined` when the slug cannot back a schema.
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
 * `<dir>/oxy-app.json`, or `undefined` when there is none. A broken one throws:
 * publishing past it used to ship a bundle with no functions and an identity
 * taken from flags or the directory.
 */
export function loadPublishManifest(dir: string): PublishManifest | undefined {
  return readAppManifest(dir) as PublishManifest | undefined;
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

/** Source entry relative to the app directory. Default `functions/<name>.ts`. */
export function functionEntry(manifest: PublishManifest | undefined, name: string): string {
  return orDefault(manifest?.functions?.[name]?.entry, `functions/${name}.ts`);
}

/**
 * `^[a-z][a-z0-9-]{0,63}$` — the server's `is_valid_function_name`.
 *
 * A manifest key becomes `functions/<name>.js`, so `../../x` has to be refused
 * before esbuild is told to write there.
 */
export function isValidFunctionName(name: string): boolean {
  return /^[a-z][a-z0-9-]{0,63}$/.test(name);
}

/**
 * The server's `is_valid_slug`: 1–63 lowercase letters, digits and single
 * hyphens, no leading or trailing hyphen.
 *
 * Checked before anything is built, so an author whose bundle never went
 * through the Vite plugin learns about it before uploading rather than after.
 */
export function isValidSlug(slug: string): boolean {
  return (
    slug.length > 0 &&
    slug.length <= 63 &&
    /^[a-z0-9-]+$/.test(slug) &&
    !slug.startsWith("-") &&
    !slug.endsWith("-") &&
    !slug.includes("--")
  );
}
