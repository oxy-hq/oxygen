import type { ExperimentDefinition } from "@/types/experiment";

/** Older stored designs may lack any key, so every read of the definition goes through this shape. */
export type DeepPartial<T> = T extends readonly unknown[]
  ? T
  : T extends object
    ? { [K in keyof T]?: DeepPartial<T[K]> }
    : T;

export type SafeDefinition = DeepPartial<ExperimentDefinition>;

export const DASH = "—";

/** Format `v` with `f`, or a dash when it is missing. */
export const orDash = <T>(v: T | null | undefined, f: (v: T) => string = String): string =>
  v === null || v === undefined ? DASH : f(v);

/** A definition that is an object, or an empty one; the server's JSON is never trusted to be whole. */
export function readDefinition(raw: unknown): SafeDefinition {
  return typeof raw === "object" && raw !== null && !Array.isArray(raw)
    ? (raw as SafeDefinition)
    : {};
}
