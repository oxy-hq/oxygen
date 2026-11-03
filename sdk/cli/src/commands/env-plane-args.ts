/**
 * The argument grammar of `oxyc env create|update|reset`'s plane flags, with
 * no I/O — shared by the CLI verbs and the `oxyc mcp` tools so neither is a
 * second implementation of the validation.
 *
 * A plane's value on the wire is its mode string, or an object
 * `{"mode": "<mode>", …settings}` for a plane that has settings
 * (`internal-docs/2026-11-02-custom-app-branches-p1-plan.md`, A4). `--plane`
 * carries only the string form; `--destination` and `--fetch-host` build the
 * warehouse and fetch planes' objects. An MCP tool may pass either. The server
 * owns which keys and modes exist (`400 invalid_planes`), so only the shape is
 * checked here.
 */

import { usageError } from "../util/errors.js";

/** A plane's wire value: a mode, or `{mode, …settings}`. */
export type PlaneValue = string | Record<string, unknown>;

/** The `--plane <key>=default` spelling that removes a key from `planes_set`. */
export const DEFAULT_MODE = "default";

export const PRESETS = ["light", "full"] as const;
export type Preset = (typeof PRESETS)[number];

const PLANE_KEY_RE = /^[a-z][a-z0-9_-]*$/;
const PLANE_HINT = "e.g. --plane storage=empty --plane oltp=schema";

function requirePlaneKey(key: string, flag: string): string {
  if (!PLANE_KEY_RE.test(key)) {
    throw usageError(
      `${flag}: ${JSON.stringify(key)} is not a plane key`,
      "a plane key is lowercase letters, digits, hyphens and underscores, e.g. storage, oltp, workspace-actions"
    );
  }
  return key;
}

/**
 * `--plane key=mode` (repeatable) into `{key: mode}`. A key given twice keeps
 * its last mode, as every repeated flag here does.
 */
export function parsePlaneFlags(values: readonly string[]): Record<string, string> {
  const planes: Record<string, string> = {};
  for (const raw of values) {
    const eq = raw.indexOf("=");
    const key = eq < 0 ? raw : raw.slice(0, eq);
    const mode = eq < 0 ? "" : raw.slice(eq + 1);
    if (eq < 0 || key === "" || mode === "") {
      throw usageError(`--plane ${JSON.stringify(raw)} is not <key>=<mode>`, PLANE_HINT);
    }
    planes[requirePlaneKey(key, "--plane")] = mode;
  }
  return planes;
}

/**
 * A `planes` object that arrived as data (an MCP argument): a plain object
 * whose values are non-empty mode strings or `{mode, …}` objects.
 */
export function validatePlaneMap(value: unknown): Record<string, PlaneValue> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw usageError(
      "planes must be an object of plane key to mode",
      'e.g. {"storage": "own", "oltp": "schema"}'
    );
  }
  const planes: Record<string, PlaneValue> = {};
  for (const [key, mode] of Object.entries(value)) {
    requirePlaneKey(key, "planes");
    const isMode = typeof mode === "string" && mode !== "";
    const isSettings = mode !== null && typeof mode === "object" && !Array.isArray(mode);
    if (!isMode && !isSettings) {
      throw usageError(
        `planes.${key} must be a mode string or a {mode, …} object`,
        'e.g. "own", or "default" to clear the key'
      );
    }
    planes[key] = mode as PlaneValue;
  }
  return planes;
}

/** The warehouse plane's per-environment overrides: what to set and what to drop. */
export interface DestinationFlags {
  /** Production database to the database this environment writes instead. */
  set: Record<string, string>;
  /** Production databases whose override is removed. */
  clear: string[];
}

const DESTINATION_HINT = "e.g. --destination clickhouse=clickhouse_dev";

/**
 * `--destination <db>=<mapped>` (repeatable) and `--clear-destination <db>`
 * (repeatable) into the warehouse plane's override edits. Both names must be
 * given; the server owns whether they are databases of the workspace
 * (`400 invalid_planes`). A database set and cleared in one command is
 * ambiguous, so it is refused.
 */
export function parseDestinationFlags(
  set: readonly string[],
  clear: readonly string[]
): DestinationFlags {
  const overrides: Record<string, string> = {};
  for (const raw of set) {
    const eq = raw.indexOf("=");
    const db = eq < 0 ? "" : raw.slice(0, eq).trim();
    const mapped = eq < 0 ? "" : raw.slice(eq + 1).trim();
    if (db === "" || mapped === "") {
      throw usageError(
        `--destination ${JSON.stringify(raw)} is not <db>=<mapped>`,
        `${DESTINATION_HINT}; to remove an override use --clear-destination <db>`
      );
    }
    overrides[db] = mapped;
  }
  const cleared = [...new Set(clear.map((db) => db.trim()))];
  if (cleared.some((db) => db === "")) {
    throw usageError(
      "--clear-destination needs a database name",
      "e.g. --clear-destination clickhouse"
    );
  }
  const both = cleared.find((db) => db in overrides);
  if (both !== undefined) {
    throw usageError(
      `${JSON.stringify(both)} is both set (--destination) and cleared (--clear-destination)`,
      "pass one of them"
    );
  }
  return { set: overrides, clear: cleared };
}

/** Whether the flags change nothing. */
export function noDestinationEdits(flags: DestinationFlags | undefined): boolean {
  return !flags || (Object.keys(flags.set).length === 0 && flags.clear.length === 0);
}

/**
 * `planes` with the warehouse overrides edited: `set` replaces an override key
 * by key and `clear` removes one, leaving the others. The warehouse plane is
 * the bare mode string or `{mode, destinations}` on the wire; either is read,
 * and the result is the object form, or the bare mode once no override is
 * left. A plane left alone is not touched.
 */
export function applyDestinations(
  planes: Record<string, unknown>,
  flags: DestinationFlags
): Record<string, unknown> {
  if (noDestinationEdits(flags)) return planes;
  const current = planes.warehouse;
  const object = current !== null && typeof current === "object" && !Array.isArray(current);
  const mode = object ? ((current as { mode?: unknown }).mode ?? "mapped") : (current ?? "mapped");
  const before = object ? ((current as { destinations?: unknown }).destinations ?? {}) : {};
  const destinations: Record<string, string> = {
    ...(before as Record<string, string>),
    ...flags.set
  };
  for (const db of flags.clear) delete destinations[db];
  // Clearing what was never set leaves the plane as it was.
  if (current === undefined && Object.keys(destinations).length === 0) return planes;
  const warehouse = Object.keys(destinations).length === 0 ? mode : { mode, destinations };
  return { ...planes, warehouse };
}

const FETCH_HOST_HINT = "e.g. --plane fetch=allow --fetch-host api.stripe.com";

/**
 * `--fetch-host <host>` (repeatable) into a lowercased, de-duplicated list.
 * The server owns which hosts are registered provider sandboxes
 * (`400 invalid_planes`); only an empty name or one carrying a scheme, path or
 * port is refused here.
 */
export function parseFetchHosts(values: readonly string[]): string[] {
  const hosts = values.map((host) => host.trim().toLowerCase());
  const bad = hosts.find((host) => host === "" || /[/:\s]/.test(host));
  if (bad !== undefined) {
    throw usageError(
      `--fetch-host ${JSON.stringify(bad)} is not a host name`,
      `a bare host, no scheme or path: ${FETCH_HOST_HINT}`
    );
  }
  return [...new Set(hosts)];
}

/** The mode a plane's wire value names: the string itself, or the object's `mode`. */
function modeOf(value: unknown): unknown {
  if (value !== null && typeof value === "object" && !Array.isArray(value)) {
    return (value as { mode?: unknown }).mode;
  }
  return value;
}

/**
 * `planes` with the fetch plane's host list applied. `hosts` replaces the
 * whole list, so what is sent is exactly what is opened, and implies
 * `fetch=allow`; naming another fetch mode beside it is refused. A bare
 * `fetch=allow` with no hosts keeps `currentFetch`'s list when that already
 * allows some, and is otherwise refused, since the server would hold every
 * call. A plane left alone is not touched.
 */
export function applyFetchHosts(
  planes: Record<string, unknown>,
  hosts: readonly string[],
  currentFetch?: unknown
): Record<string, unknown> {
  const mode = modeOf(planes.fetch);
  if (hosts.length > 0) {
    if (mode !== undefined && mode !== "allow") {
      const shown = typeof mode === "string" ? mode : JSON.stringify(mode);
      throw usageError(
        `--fetch-host needs fetch=allow, not fetch=${shown}`,
        `drop --plane fetch=${shown}, or drop --fetch-host: ${FETCH_HOST_HINT}`
      );
    }
    return { ...planes, fetch: { mode: "allow", hosts: [...hosts] } };
  }
  if (planes.fetch !== "allow") return planes;
  const kept = (currentFetch as { hosts?: unknown } | null | undefined)?.hosts;
  if (modeOf(currentFetch) === "allow" && Array.isArray(kept) && kept.length > 0) {
    return { ...planes, fetch: currentFetch };
  }
  throw usageError("fetch=allow needs at least one --fetch-host", FETCH_HOST_HINT);
}

/** `--preset`: `light` or `full`. */
export function requirePreset(value: string): Preset {
  if ((PRESETS as readonly string[]).includes(value)) return value as Preset;
  throw usageError(`--preset ${JSON.stringify(value)} is not a preset`, "light or full");
}

/** `--at`: any date `Date` can read, sent as an ISO 8601 / RFC 3339 instant. */
export function requireForkAt(value: string): string {
  const parsed = new Date(value);
  if (value.trim() === "" || Number.isNaN(parsed.getTime())) {
    throw usageError(
      `--at ${JSON.stringify(value)} is not a date`,
      "e.g. --at 2026-11-01T09:00:00Z, or a date: --at 2026-11-01"
    );
  }
  return parsed.toISOString();
}

/**
 * The object `env update` sends. Merging keeps what is already set and lets the
 * flags win; `replace` sends the flags alone. Either way `<key>=default` drops
 * the key, which is how a plane returns to its default.
 */
export function mergePlanes(
  current: Record<string, unknown> | null | undefined,
  flags: Record<string, PlaneValue>,
  replace: boolean
): Record<string, unknown> {
  const merged: Record<string, unknown> = replace ? {} : { ...(current ?? {}) };
  for (const [key, mode] of Object.entries(flags)) {
    if (mode === DEFAULT_MODE) delete merged[key];
    else merged[key] = mode;
  }
  return merged;
}

/** `--plane a,b` (repeatable) into a de-duplicated key list. */
export function parsePlaneKeys(values: readonly string[]): string[] {
  const keys = values
    .flatMap((value) => value.split(","))
    .map((key) => key.trim())
    .filter((key) => key !== "");
  if (keys.length === 0) {
    throw usageError("name at least one plane to reset", "e.g. --plane storage,oltp");
  }
  return [...new Set(keys.map((key) => requirePlaneKey(key, "--plane")))];
}

/** `{a: "own", b: {mode: "mapped"}}` as `a=own b=mapped…`, for the human output. */
export function describePlanesSet(set: Record<string, unknown> | null | undefined): string | null {
  if (!set) return null;
  const parts = Object.entries(set).map(([key, value]) => {
    if (typeof value === "string") return `${key}=${value}`;
    const mode = (value as { mode?: unknown } | null)?.mode;
    return `${key}=${typeof mode === "string" ? mode : "?"}…`;
  });
  return parts.length === 0 ? null : parts.join(" ");
}
