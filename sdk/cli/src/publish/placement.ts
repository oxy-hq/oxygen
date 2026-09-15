/**
 * The data-placement checks `oxyc validate` runs over an `oxy-app.json`.
 *
 * THE SHAPE OF THE DATA PICKS THE STORE. Append-only facts and history go to
 * the workspace's Airhouse through `ctx.airhouse`, in the app's own
 * `app_<writer>` schema; records the app edits go to org OLTP (`ctx.oltp`);
 * files go to `ctx.storage`; a customer warehouse is read-only, written only
 * under a declared, reasoned exception.
 *
 * ADVISORY. Each rule mirrors a refusal the platform makes at publish or
 * promote, and answers before a bundle is built. The server is the authority:
 * where the two disagree, it is right.
 */

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

import {
  appWriterName,
  functionEntry,
  type PublishManifest,
  WRITER_NAME_RULE
} from "./manifest.js";
import { scanAirhouseMigration, secretsUsedAsState } from "./placement-scan.js";

export interface PlacementIssue {
  level: "error" | "warning";
  /** Relative to the app directory: `oxy-app.json`, a function's source, or a migration. */
  file: string;
  /** A manifest key path, or `line N`. */
  path: string;
  message: string;
}

const MANIFEST = "oxy-app.json";

const isObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const error = (file: string, path: string, message: string): PlacementIssue => ({
  level: "error",
  file,
  path,
  message
});

const warning = (file: string, path: string, message: string): PlacementIssue => ({
  level: "warning",
  file,
  path,
  message
});

/** The writer the platform will derive from the slug, or why it cannot. */
type Writer =
  | { kind: "derived"; schema: string }
  | { kind: "underivable"; why: string }
  | { kind: "unknown" };

function writerFor(slug: string | undefined): Writer {
  if (slug === undefined) return { kind: "unknown" };
  const name = appWriterName(slug);
  if (name !== undefined) return { kind: "derived", schema: `app_${name}` };
  return {
    kind: "underivable",
    why: slug.includes("_")
      ? `the slug \`${slug}\` contains \`_\`, and a slug containing \`_\` cannot back a ` +
        "schema — it would share one with its hyphenated twin"
      : `the slug \`${slug}\` cannot derive a writer: ${WRITER_NAME_RULE}`
  };
}

/**
 * Every placement issue in one app.
 *
 * `slug` is passed rather than read, because a manifest without one still
 * publishes — under `--app`, or from an `apps/<org>/<app>/` directory — and the
 * caller is what knows which applies.
 */
export function checkAppPlacement(
  appDir: string,
  manifest: Record<string, unknown>,
  slug: string | undefined
): PlacementIssue[] {
  const issues: PlacementIssue[] = [];
  const writer = writerFor(slug);
  let usesSchema = false;

  const functions = isObject(manifest.functions) ? manifest.functions : {};
  for (const [name, spec] of Object.entries(functions)) {
    if (!isObject(spec)) {
      issues.push(error(MANIFEST, `functions.${name}`, "must be an object"));
      continue;
    }
    checkWarehouseWrites(name, spec, issues);
    usesSchema = checkAirhouseGate(name, spec, writer, issues) || usesSchema;
    checkSecretsAsState(appDir, manifest, name, spec, issues);
  }
  usesSchema = checkAirhouseMigrations(appDir, manifest, writer, issues) || usesSchema;

  if (usesSchema && writer.kind === "unknown") {
    issues.push(
      warning(
        MANIFEST,
        "slug",
        "`ctx.airhouse` and Airhouse migrations use the schema `app_<writer>`, derived from the " +
          "app slug, and no slug is known here — set `slug` in oxy-app.json so it can be checked"
      )
    );
  }
  return issues;
}

function checkWarehouseWrites(
  fn: string,
  spec: Record<string, unknown>,
  issues: PlacementIssue[]
): void {
  const writes = spec.customerWarehouseWrites;
  if (writes == null) return;
  const path = `functions.${fn}.customerWarehouseWrites`;
  if (!isObject(writes)) {
    issues.push(
      error(
        MANIFEST,
        path,
        "must be an object mapping a database to the reason this function may write to it"
      )
    );
    return;
  }
  const destinations = Array.isArray(spec.destinations) ? spec.destinations : [];
  for (const [database, reason] of Object.entries(writes)) {
    const at = `${path}.${database}`;
    if (typeof reason !== "string" || !reason.trim()) {
      issues.push(
        error(
          MANIFEST,
          at,
          "the reason must be a non-empty string — an exception to read-only customer " +
            "warehouses has to say why it exists"
        )
      );
    } else if (!destinations.includes(database)) {
      issues.push(
        error(
          MANIFEST,
          at,
          `\`${database}\` is not in this function's \`destinations\`, so the function cannot ` +
            "write to it — an exception for a database the function cannot write is dead config"
        )
      );
    } else {
      issues.push(
        warning(
          MANIFEST,
          at,
          `function \`${fn}\` writes to customer warehouse \`${database}\` (reason: ` +
            `${reason.trim()}) — customer warehouses are read-only by default; facts and ` +
            "history belong in `ctx.airhouse`, records in `ctx.oltp`"
        )
      );
    }
  }
}

/** True when this function turns `ctx.airhouse` on. */
function checkAirhouseGate(
  fn: string,
  spec: Record<string, unknown>,
  writer: Writer,
  issues: PlacementIssue[]
): boolean {
  const gate = spec.airhouse;
  if (gate == null) return false;
  const path = `functions.${fn}.airhouse`;
  if (!isObject(gate)) {
    issues.push(error(MANIFEST, path, 'must be an object: `{ "enabled": true }`'));
    return false;
  }
  if (gate.enabled != null && typeof gate.enabled !== "boolean") {
    issues.push(error(MANIFEST, `${path}.enabled`, "must be true or false"));
    return false;
  }
  if (gate.enabled !== true) return false;
  if (writer.kind === "underivable") {
    issues.push(
      error(
        MANIFEST,
        `${path}.enabled`,
        "`ctx.airhouse` writes the app's own schema `app_<writer>`, whose writer is derived " +
          `from the app slug — and ${writer.why}`
      )
    );
  }
  return true;
}

function checkSecretsAsState(
  appDir: string,
  manifest: Record<string, unknown>,
  fn: string,
  spec: Record<string, unknown>,
  issues: PlacementIssue[]
): void {
  if (spec.entry != null && typeof spec.entry !== "string") {
    issues.push(error(MANIFEST, `functions.${fn}.entry`, "must be a string"));
    return;
  }
  const entry = functionEntry(manifest as PublishManifest, fn);
  let source: string;
  try {
    source = readFileSync(join(appDir, entry), "utf8");
  } catch (cause) {
    const code = (cause as NodeJS.ErrnoException).code ?? "EUNKNOWN";
    issues.push(
      warning(
        MANIFEST,
        `functions.${fn}`,
        `not scanned for \`ctx.secrets.set\`: ${entry} could not be read (${code})`
      )
    );
    return;
  }
  for (const hit of secretsUsedAsState(source)) {
    issues.push(
      warning(
        entry,
        `line ${hit.line}`,
        `function \`${fn}\` stores ${hit.why} with \`${hit.call}\` — secrets hold credentials; ` +
          "state that changes belongs in `ctx.oltp` (records) or `ctx.airhouse` (facts)"
      )
    );
  }
}

/**
 * `migrations.dir`'s rule on the server (`normalize_dir`): a plain relative
 * directory, since it is matched as a prefix inside the bundle.
 */
function normalizeDir(dir: string): string | undefined {
  const trimmed = dir.trim().replace(/^\/+|\/+$/g, "");
  if (
    trimmed === "" ||
    dir.startsWith("/") ||
    dir.includes("\\") ||
    trimmed.split("/").some((c) => c === ".." || c === "." || c === "")
  ) {
    return undefined;
  }
  return trimmed;
}

/** The declared directory, normalised, or `undefined` after reporting why not. */
function declaredDir(
  manifest: Record<string, unknown>,
  issues: PlacementIssue[]
): string | undefined {
  const block = manifest.airhouseMigrations;
  if (!isObject(block)) {
    issues.push(
      error(MANIFEST, "airhouseMigrations", 'must be an object: `{ "dir": "airhouse-migrations" }`')
    );
    return undefined;
  }
  if (typeof block.dir !== "string" || !block.dir.trim()) {
    issues.push(
      error(
        MANIFEST,
        "airhouseMigrations.dir",
        "is required — the directory of `.sql` files applied, once each and in filename " +
          "order, to the app's Airhouse schema at promote"
      )
    );
    return undefined;
  }
  const dir = normalizeDir(block.dir);
  if (dir === undefined) {
    issues.push(
      error(
        MANIFEST,
        "airhouseMigrations.dir",
        `\`${block.dir}\` is not a safe path inside the bundle — use a plain relative directory ` +
          'such as "airhouse-migrations"'
      )
    );
    return undefined;
  }
  const oltp = manifest.migrations;
  if (isObject(oltp) && typeof oltp.dir === "string" && normalizeDir(oltp.dir) === dir) {
    issues.push(
      error(
        MANIFEST,
        "airhouseMigrations.dir",
        `\`${dir}\` is also \`migrations.dir\` — OLTP migrations run on Postgres and Airhouse ` +
          "migrations on DuckLake, so each needs its own directory"
      )
    );
    return undefined;
  }
  return dir;
}

/**
 * Where the directory is in the SOURCE tree.
 *
 * The server reads it from the built bundle, so it may live at the app root,
 * under `public/` (which Vite copies to the bundle root), or already in the
 * build output.
 */
function locateDir(appDir: string, dir: string, outDir: string): string | undefined {
  const candidates = new Set([dir, `public/${dir}`, `${outDir.replace(/\/+$/, "")}/${dir}`]);
  for (const candidate of candidates) {
    try {
      if (statSync(join(appDir, candidate)).isDirectory()) return candidate;
    } catch {
      // Not here; the next candidate may be.
    }
  }
  return undefined;
}

/** `*.sql` under `root`, as paths relative to it — the server's `collect`, which recurses. */
function sqlFiles(root: string, prefix = ""): string[] {
  const found: string[] = [];
  for (const entry of readdirSync(join(root, prefix), { withFileTypes: true })) {
    const rel = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isDirectory()) found.push(...sqlFiles(root, rel));
    // Case-sensitive, as the server's is.
    else if (entry.name.endsWith(".sql")) found.push(rel);
  }
  return found;
}

/** True when the app declares Airhouse migrations. */
function checkAirhouseMigrations(
  appDir: string,
  manifest: Record<string, unknown>,
  writer: Writer,
  issues: PlacementIssue[]
): boolean {
  if (manifest.airhouseMigrations == null) return false;
  const dir = declaredDir(manifest, issues);
  if (dir === undefined) return true;
  if (writer.kind === "underivable") {
    issues.push(
      error(
        MANIFEST,
        "airhouseMigrations",
        "Airhouse migrations run in the app's own schema `app_<writer>`, whose writer is " +
          `derived from the app slug — and ${writer.why}`
      )
    );
  }

  const build = isObject(manifest.build) ? manifest.build : {};
  const outDir = typeof build.outDir === "string" && build.outDir.trim() ? build.outDir : "out";
  const found = locateDir(appDir, dir, outDir);
  if (found === undefined) {
    issues.push(
      error(
        MANIFEST,
        "airhouseMigrations.dir",
        `\`${dir}\` does not exist — looked for ${dir}/, public/${dir}/ and ${outDir}/${dir}/ ` +
          "in the app directory"
      )
    );
    return true;
  }

  let files: string[];
  try {
    files = sqlFiles(join(appDir, found)).sort();
  } catch (cause) {
    issues.push(
      error(
        MANIFEST,
        "airhouseMigrations.dir",
        `\`${found}\` could not be read: ${(cause as Error).message}`
      )
    );
    return true;
  }
  if (files.length === 0) {
    issues.push(
      error(
        MANIFEST,
        "airhouseMigrations.dir",
        `\`${found}\` holds no \`.sql\` files — the platform refuses a declared migrations ` +
          "directory with nothing in it"
      )
    );
    return true;
  }

  const schema = writer.kind === "derived" ? writer.schema : undefined;
  for (const rel of files) {
    const file = `${found}/${rel}`;
    const sql = readFileSync(join(appDir, file), "utf8");
    for (const hit of scanAirhouseMigration(sql, schema)) {
      issues.push({ level: hit.level, file, path: `line ${hit.line}`, message: hit.message });
    }
  }
  return true;
}
