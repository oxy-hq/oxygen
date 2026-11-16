import type { Secret } from "@/types/secret";
import type { SharedSecret } from "@/types/sharedSecrets";
import { parseAppSecretName } from "./appSecretName";
import type { AppRef, SecretsFilter, SecretsScope, UnifiedRow } from "./types";

/** App id → display name. An app the map lacks falls back to a short id —
 *  still better than the full uuid inline in the name column. */
export type AppNames = Map<string, string>;

/** Enough of a uuid to tell two apps apart when neither has a resolved name. */
const shortId = (id: string) => id.slice(0, 8);

const appRef = (id: string, names: AppNames): AppRef => ({
  id,
  name: names.get(id) ?? shortId(id)
});

/**
 * One row per stored secret. A workspace secret carries the apps it is shared
 * with; an app's own secret (`apps/<app_id>/<KEY>`) is shown as the key that app
 * reads, attributed to it — the raw storage name made those rows unidentifiable.
 *
 * Workspace secrets first, then each app's own, grouped by app: an app's keys are
 * a set someone reads together.
 */
export function buildRows(
  secrets: Secret[],
  shared: SharedSecret[],
  names: AppNames
): UnifiedRow[] {
  const sharedWith = new Map(shared.map((s) => [s.secret_id, s.app_ids]));
  const rows: UnifiedRow[] = secrets.map((secret) => {
    const scoped = parseAppSecretName(secret.name);
    if (scoped) {
      return {
        key: `secret-${secret.id}`,
        name: scoped.key,
        app: appRef(scoped.appId, names),
        sharedWith: [],
        secretInfo: secret
      };
    }
    const apps = (sharedWith.get(secret.id) ?? []).map((id) => appRef(id, names));
    apps.sort((a, b) => a.name.localeCompare(b.name));
    return { key: `secret-${secret.id}`, name: secret.name, sharedWith: apps, secretInfo: secret };
  });
  rows.sort((a, b) => {
    const group = (a.app?.name ?? "").localeCompare(b.app?.name ?? "");
    return group !== 0 ? group : a.name.localeCompare(b.name);
  });
  return rows;
}

/** Which tab a row lives in. */
export const scopeOf = (row: UnifiedRow): SecretsScope => (row.app ? "app" : "workspace");

/** The rows of one tab. */
export const inScope = (rows: UnifiedRow[], scope: SecretsScope): UnifiedRow[] =>
  rows.filter((row) => scopeOf(row) === scope);

/** Whether any app reads this workspace secret. */
export const kindOf = (row: UnifiedRow): Exclude<SecretsFilter, "all"> =>
  row.sharedWith.length > 0 ? "shared" : "unshared";

/** Everything the search box matches a row on. */
const haystack = (row: UnifiedRow): string[] => [
  row.name,
  row.secretInfo.description ?? "",
  row.app?.name ?? "",
  ...row.sharedWith.map((a) => a.name)
];

/**
 * The rows left after the tab's own narrowing and the search `query`.
 *
 * In the Workspace tab `filter` is whether apps read it; in the Custom apps tab
 * `appId` is one app. The query matches a secret's name, its description, and
 * the name of any app that reads it — "which secrets does Platform Pulse get?"
 * is a search for the app.
 */
export function filterRows(
  rows: UnifiedRow[],
  { filter = "all", appId }: { filter?: SecretsFilter; appId?: string },
  query: string
): UnifiedRow[] {
  const q = query.trim().toLowerCase();
  return rows.filter((row) => {
    if (filter !== "all" && kindOf(row) !== filter) return false;
    if (appId && row.app?.id !== appId) return false;
    if (!q) return true;
    return haystack(row).some((text) => text.toLowerCase().includes(q));
  });
}

/** How many rows each Workspace-tab filter would keep — for the filter labels. */
export function countByKind(rows: UnifiedRow[]): Record<SecretsFilter, number> {
  const counts: Record<SecretsFilter, number> = { all: rows.length, shared: 0, unshared: 0 };
  for (const row of rows) counts[kindOf(row)] += 1;
  return counts;
}

/** How many rows each app owns — for the app filter's labels. */
export function countByApp(rows: UnifiedRow[]): Map<string, number> {
  const counts = new Map<string, number>();
  for (const row of rows) {
    if (!row.app) continue;
    counts.set(row.app.id, (counts.get(row.app.id) ?? 0) + 1);
  }
  return counts;
}

/** Every app that reads `row`, by name — what deleting it would take away. */
export const readersOf = (row: UnifiedRow): string[] =>
  row.app ? [row.app.name] : row.sharedWith.map((a) => a.name);
