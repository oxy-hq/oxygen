import type { Secret } from "@/types/secret";
import type { SharedSecret } from "@/types/sharedSecrets";
import { parseAppSecretName } from "./appSecretName";
import type { AppRef, SecretsFilter, UnifiedRow } from "./types";

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

/** Which filter a row falls under, besides "all". */
export function kindOf(row: UnifiedRow): Exclude<SecretsFilter, "all"> {
  if (row.app) return "app";
  return row.sharedWith.length > 0 ? "shared" : "workspace";
}

/**
 * The rows `filter` and the search `query` keep. The query matches a secret's
 * name, its description, and the name of any app that reads it — "which secrets
 * does Platform Pulse get?" is a search for the app.
 */
export function filterRows(rows: UnifiedRow[], filter: SecretsFilter, query: string): UnifiedRow[] {
  const q = query.trim().toLowerCase();
  return rows.filter((row) => {
    if (filter !== "all" && kindOf(row) !== filter) return false;
    if (!q) return true;
    const haystack = [
      row.name,
      row.secretInfo.description ?? "",
      row.app?.name ?? "",
      ...row.sharedWith.map((a) => a.name)
    ];
    return haystack.some((text) => text.toLowerCase().includes(q));
  });
}

/** How many rows each filter would keep — for the filter's labels. */
export function countByKind(rows: UnifiedRow[]): Record<SecretsFilter, number> {
  const counts: Record<SecretsFilter, number> = {
    all: rows.length,
    shared: 0,
    workspace: 0,
    app: 0
  };
  for (const row of rows) counts[kindOf(row)] += 1;
  return counts;
}
