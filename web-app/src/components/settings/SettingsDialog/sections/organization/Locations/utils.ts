import type { LocationRow, LocationStatus, UpdateLocationRequest } from "@/types/operatingGraph";

export const LOCATION_STATUSES: LocationStatus[] = [
  "pre_launch",
  "launching",
  "open",
  "archived",
  "terminated"
];

export const LOCATION_STATUS_LABELS: Record<LocationStatus, string> = {
  pre_launch: "Pre-launch",
  launching: "Launching",
  open: "Open",
  archived: "Archived",
  terminated: "Terminated"
};

export interface LocationTreeRow {
  location: LocationRow;
  depth: number;
}

/**
 * The org's locations as a flattened tree: roots first, each followed by its
 * children, siblings by name. A location whose parent is missing from the
 * list — deleted, or not yet loaded — renders as a root rather than vanishing,
 * and a cycle the server should have refused can't recurse forever.
 */
export function locationTree(locations: LocationRow[]): LocationTreeRow[] {
  const ids = new Set(locations.map((l) => l.id));
  const children = new Map<string | null, LocationRow[]>();
  for (const location of locations) {
    const parent =
      location.parent_id !== null && ids.has(location.parent_id) ? location.parent_id : null;
    const siblings = children.get(parent);
    if (siblings) siblings.push(location);
    else children.set(parent, [location]);
  }
  const byName = (a: LocationRow, b: LocationRow) => a.name.localeCompare(b.name);
  const out: LocationTreeRow[] = [];
  const seen = new Set<string>();
  const visit = (parent: string | null, depth: number) => {
    for (const location of (children.get(parent) ?? []).sort(byName)) {
      if (seen.has(location.id)) continue;
      seen.add(location.id);
      out.push({ location, depth });
      visit(location.id, depth + 1);
    }
  };
  visit(null, 0);
  // Anything unreached sits in a cycle with no root: show it flat, don't lose it.
  for (const location of [...locations].sort(byName)) {
    if (!seen.has(location.id)) {
      seen.add(location.id);
      out.push({ location, depth: 0 });
    }
  }
  return out;
}

/**
 * A location and everything under it. These are the ids the parent picker
 * must not offer: choosing any of them would make the location its own
 * ancestor, which the server refuses with a 400 — better never to offer it.
 */
export function descendantIds(locations: LocationRow[], id: string): Set<string> {
  const childrenOf = new Map<string, string[]>();
  for (const location of locations) {
    if (location.parent_id === null) continue;
    const list = childrenOf.get(location.parent_id);
    if (list) list.push(location.id);
    else childrenOf.set(location.parent_id, [location.id]);
  }
  const out = new Set<string>([id]);
  const stack = [id];
  while (stack.length > 0) {
    const next = stack.pop() as string;
    for (const child of childrenOf.get(next) ?? []) {
      if (out.has(child)) continue;
      out.add(child);
      stack.push(child);
    }
  }
  return out;
}

/** The level names this org already uses, for the kind field's suggestions. */
export function usedKinds(locations: LocationRow[]): string[] {
  const kinds = new Set<string>();
  for (const location of locations) {
    if (location.kind) kinds.add(location.kind);
  }
  return [...kinds].sort((a, b) => a.localeCompare(b));
}

/** "3 locations · 2 open" — the section's one-line summary. */
export function locationSummary(locations: LocationRow[]): string {
  const total = locations.length;
  const open = locations.filter((l) => l.status === "open").length;
  return `${total} ${total === 1 ? "location" : "locations"} · ${open} open`;
}

// ── The form's draft → what to send ──

/** What the location form holds; the same shape whether creating or editing. */
export interface LocationDraft {
  name: string;
  kind: string;
  /** `null` for top level. */
  parentId: string | null;
  status: LocationStatus;
  timezone: string;
  /** The tenant's own id (`external_id`); blank means none. */
  externalId: string;
}

/**
 * The PATCH that turns `row` into `draft` — only the fields that differ, so a
 * save that changed nothing sends nothing. Values are compared as the server
 * stores them: trimmed, a kind lowercased, a blank id as `null`.
 */
export function locationPatch(row: LocationRow, draft: LocationDraft): UpdateLocationRequest {
  const patch: UpdateLocationRequest = {};
  const name = draft.name.trim();
  if (name !== row.name) patch.name = name;
  const kind = draft.kind.trim().toLowerCase() || null;
  if (kind !== row.kind) patch.kind = kind;
  if (draft.parentId !== row.parent_id) patch.parent_id = draft.parentId;
  if (draft.status !== row.status) patch.status = draft.status;
  if (draft.timezone !== row.timezone) patch.timezone = draft.timezone;
  const externalId = draft.externalId.trim() || null;
  if (externalId !== row.external_id) patch.external_id = externalId;
  return patch;
}

// ── External ids ──

/** The server's rule for a system token, mirrored so a bad one never leaves the form. */
export const SYSTEM_PATTERN = /^[a-z0-9_-]{1,32}$/;

export interface ExternalIdDraft {
  /** Stable React key while the row is edited; not sent anywhere. */
  key: number;
  system: string;
  id: string;
}

/** Why one system token can't be sent, or null when it can. */
export function systemProblem(system: string): string | null {
  if (!SYSTEM_PATTERN.test(system)) {
    return "A system is 1 to 32 lowercase letters, digits, - or _ (like toast or unifi).";
  }
  return null;
}

/** Why the editor's rows can't be sent as a whole, or null when they can. */
export function externalIdsProblem(rows: ExternalIdDraft[]): string | null {
  const seen = new Set<string>();
  for (const row of rows) {
    const problem = systemProblem(row.system.trim());
    if (problem) return problem;
    if (!row.id.trim()) return `Enter the id ${row.system.trim()} uses for this location.`;
    if (seen.has(row.system.trim())) return `${row.system.trim()} is listed twice.`;
    seen.add(row.system.trim());
  }
  return null;
}

export function draftsToRecord(rows: ExternalIdDraft[]): Record<string, string> {
  const out: Record<string, string> = {};
  for (const row of rows) out[row.system.trim()] = row.id.trim();
  return out;
}

export function recordToDrafts(record: Record<string, string>): ExternalIdDraft[] {
  return Object.entries(record)
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([system, id], key) => ({ key, system, id }));
}

export interface ExternalIdDiff {
  /** Systems to PUT: added, or with a changed id. */
  set: Array<[system: string, id: string]>;
  /** Systems to DELETE. */
  remove: string[];
}

/** What to send so the server's map becomes `after`, given it is `before` now. */
export function externalIdDiff(
  before: Record<string, string>,
  after: Record<string, string>
): ExternalIdDiff {
  const set: ExternalIdDiff["set"] = [];
  for (const [system, id] of Object.entries(after)) {
    if (before[system] !== id) set.push([system, id]);
  }
  const remove = Object.keys(before).filter((system) => !(system in after));
  return { set, remove };
}

// ── Timezones ──

export const TIMEZONES: string[] = (() => {
  if ("supportedValuesOf" in Intl) {
    return (Intl as unknown as { supportedValuesOf: (k: string) => string[] }).supportedValuesOf(
      "timeZone"
    );
  }
  return [
    "UTC",
    "America/New_York",
    "America/Chicago",
    "America/Denver",
    "America/Los_Angeles",
    "America/Anchorage",
    "Pacific/Honolulu",
    "Europe/London",
    "Europe/Paris",
    "Asia/Singapore",
    "Asia/Tokyo",
    "Australia/Sydney"
  ];
})();

/** The zone this browser is in — the default for a new location, which is usually nearby. */
export function browserTimeZone(): string {
  try {
    const zone = Intl.DateTimeFormat().resolvedOptions().timeZone;
    if (zone && TIMEZONES.includes(zone)) return zone;
  } catch {
    // An exotic runtime with no resolvable zone: fall through to UTC.
  }
  return "UTC";
}
