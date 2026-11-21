#!/usr/bin/env node
/**
 * Copy `json-schemas/` from the repo root into this package.
 *
 * COPIED, NOT COMMITTED. The schemas are generated from the Rust config types
 * by `just gen-schemas`, and `crates/app`'s `json_schemas_are_current`
 * test asserts the committed copies still match those types. A second
 * committed copy in here would be a second thing to keep in step, and the one
 * that silently rots — so it is gitignored and rebuilt.
 *
 * Runs as `prebuild`, so `pnpm build`, `pnpm test` (via `pretest` → `tsdown`)
 * and `prepublishOnly` all get a current set without anyone remembering.
 */

import { cpSync, existsSync, mkdirSync, readdirSync, rmSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const PKG = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const SOURCE = resolve(PKG, "..", "..", "json-schemas");
const DEST = join(PKG, "json-schemas");

if (!existsSync(SOURCE)) {
  // A published tarball has the schemas already and no repo root above it, so
  // this is a no-op there rather than an error.
  if (existsSync(DEST)) {
    console.log(`json-schemas: no repo root above ${PKG}; keeping the shipped copy`);
    process.exit(0);
  }
  console.error(`json-schemas: ${SOURCE} not found and no shipped copy to fall back on`);
  process.exit(1);
}

rmSync(DEST, { recursive: true, force: true });
mkdirSync(DEST, { recursive: true });
cpSync(SOURCE, DEST, { recursive: true });
console.log(`json-schemas: copied ${readdirSync(DEST).length} schema(s) from the repo root`);
