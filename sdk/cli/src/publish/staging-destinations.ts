/**
 * `nonProduction.destinations` for the Oxy Functions lint: a write whose
 * database the app maps is written, in staging, to the mapped database — which
 * the host holds to the same destination checks (`host/warehouse_home.rs`). So
 * the lint checks a **staging twin** of each such write too: the same call,
 * naming the mapped database, remembering the one it was mapped from.
 */

import { WRITE_OPS } from "./capabilities.js";
import type { CtxCall } from "./function-lint.js";

const isObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/**
 * The call as an author reads it: `ctx.warehouse.insert("db", …)`, and for a
 * staging twin the mapping that sent it elsewhere.
 */
export function describeWrite(call: CtxCall): string {
  if (call.database === undefined) return call.call;
  if (call.mappedFrom === undefined) return `${call.call}"${call.database}", …)`;
  return (
    `${call.call}"${call.mappedFrom}", …) in staging, which nonProduction.destinations ` +
    `maps to "${call.database}"`
  );
}

/** `nonProduction.destinations`: production database → the one staging writes. */
export function stagingDestinations(manifest: Record<string, unknown>): Record<string, string> {
  const block = isObject(manifest.nonProduction) ? manifest.nonProduction.destinations : undefined;
  if (!isObject(block)) return {};
  return Object.fromEntries(
    Object.entries(block).filter((entry): entry is [string, string] => typeof entry[1] === "string")
  );
}

/**
 * The write staging sends instead: the same call on the mapped database.
 * `undefined` when the call names no literal database, or one with no mapping
 * (staging holds that write), or a mapping onto itself (the server refuses it
 * at publish).
 */
export function stagingTwin(call: CtxCall, mapping: Record<string, string>): CtxCall | undefined {
  if (!WRITE_OPS.includes(call.member) || call.database === undefined) return undefined;
  const mapped = mapping[call.database];
  if (mapped === undefined || mapped.trim() === "" || mapped === call.database) return undefined;
  return { ...call, database: mapped, mappedFrom: call.database };
}
