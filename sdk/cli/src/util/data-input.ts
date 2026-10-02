/**
 * The `-`/`@file`/literal grammar `--data` and `--variables` share: read
 * stdin, read a file, or take the string as-is. One implementation so a
 * second flag does not quietly diverge from `fn call`'s `--data`, which
 * defined it first.
 */

import { readFileSync } from "node:fs";
import { usageError } from "./errors.js";

/** `-` is stdin, `@file` is a file, anything else is the literal value. */
export function resolveDataInput(value: string | undefined, fallback = "{}"): string {
  if (value === undefined) return fallback;
  if (value === "-") return readFileSync(0, "utf8");
  if (value.startsWith("@")) {
    const path = value.slice(1);
    try {
      return readFileSync(path, "utf8");
    } catch (cause) {
      throw usageError(`could not read ${path}: ${(cause as Error).message}`);
    }
  }
  return value;
}
