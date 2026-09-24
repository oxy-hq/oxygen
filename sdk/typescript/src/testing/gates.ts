// What the manifest grants one function — `FunctionCapabilities` in `host.rs`,
// read off the app's `oxy-app.json` the way the host and `oxyc`'s
// `declares(spec)` read it (`sdk/cli/src/publish/capabilities.ts`).
//
// THE MANIFEST IS THE WHOLE `oxy-app.json` PLUS A FUNCTION NAME
// (`internal-docs/sdk-testing-context.md` §7, answer 3): the app's `slug`
// derives the OLTP and Airhouse schema (`app_writer_name` in the `oltp` crate:
// hyphens to underscores), and `destinations` / `customerWarehouseWrites` are
// per function. Making a caller split them invites the two halves to disagree.

import { contextError } from "./host-error";

/** The loose shape a JSON import of `oxy-app.json` satisfies. */
export interface ManifestLike {
  slug?: string;
  functions?: Record<string, Record<string, unknown> | undefined>;
}

/** `FunctionCapabilities` for one function, plus its write allowlist. */
export interface FunctionGates {
  functionName: string;
  slug: string;
  /** `app_<slug>` with `-` → `_`, or `null` when the slug cannot back a writer. */
  writerSchema: string | null;
  secretsWrite: boolean;
  emailSend: boolean;
  orgRead: boolean;
  storageRead: boolean;
  storageWrite: boolean;
  oltp: boolean;
  airhouse: boolean;
  /** `destinations`: the databases `ctx.warehouse` writes and `ctx.tx` may name. */
  destinations: string[];
  /** `customerWarehouseWrites`: database → the non-blank reason, blank ones dropped. */
  customerWarehouseWrites: Record<string, string>;
  /** `fetch.maxResponseBytes`, when the entry raises the ceiling. */
  fetchMaxBytes: number | null;
}

const isObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/** `spec.<a>.<b> === true` — `flag` in `capabilities.ts`. */
function flag(spec: Record<string, unknown>, a: string, b: string): boolean {
  const block = spec[a];
  return isObject(block) && block[b] === true;
}

/**
 * `MAX_NAME_LEN` in `crates/oltp/src/schema.rs`: 63 (Postgres) less `app_` and
 * the longest role suffix `_rw`.
 */
export const MAX_SCHEMA_NAME_LEN = 56;

/**
 * `app_writer_name` + `WriterRef::schema_name`: the schema an app's slug
 * derives, or `null` when it derives none.
 *
 * Both halves matter. `app_writer_name` refuses a slug containing `_` outright
 * — `my_app` would otherwise alias the hyphenated sibling `my-app` onto one
 * schema and role — and then runs the derived name through `validate_name`,
 * which is what rejects a leading digit, an over-long name, and any character
 * outside `[a-z0-9_]`. Skipping that second half handed back a schema for slugs
 * the host answers `None` for, which is the difference between a green test and
 * a closed `ctx.oltp` in production.
 */
export function writerSchema(slug: string): string | null {
  if (slug.includes("_")) return null;
  const name = slug.replace(/-/g, "_");
  if (name === "" || name.length > MAX_SCHEMA_NAME_LEN) return null;
  if (!/^[a-z]/.test(name)) return null;
  if (!/^[a-z0-9_]+$/.test(name)) return null;
  return `app_${name}`;
}

/** Read the gates for `functionName` from `manifest`, or throw naming what is missing. */
export function readGates(manifest: ManifestLike, functionName: string): FunctionGates {
  const spec = manifest.functions?.[functionName];
  if (!isObject(spec)) {
    const known = Object.keys(manifest.functions ?? {});
    throw contextError(
      `oxy-app.json declares no function "${functionName}" (functions: ${known.length ? known.join(", ") : "none"})`
    );
  }
  const slug = typeof manifest.slug === "string" ? manifest.slug : "";
  const destinations = Array.isArray(spec.destinations)
    ? spec.destinations.filter((d): d is string => typeof d === "string")
    : [];
  const customerWarehouseWrites: Record<string, string> = {};
  if (isObject(spec.customerWarehouseWrites)) {
    for (const [db, reason] of Object.entries(spec.customerWarehouseWrites)) {
      if (typeof reason === "string" && reason.trim() !== "") customerWarehouseWrites[db] = reason;
    }
  }
  const fetch = isObject(spec.fetch) ? spec.fetch : {};
  return {
    functionName,
    slug,
    writerSchema: writerSchema(slug),
    secretsWrite: flag(spec, "secrets", "write"),
    emailSend: flag(spec, "email", "send"),
    orgRead: flag(spec, "org", "read"),
    storageRead: flag(spec, "storage", "read"),
    storageWrite: flag(spec, "storage", "write"),
    oltp: flag(spec, "oltp", "enabled"),
    airhouse: flag(spec, "airhouse", "enabled"),
    destinations,
    customerWarehouseWrites,
    fetchMaxBytes: typeof fetch.maxResponseBytes === "number" ? fetch.maxResponseBytes : null
  };
}
