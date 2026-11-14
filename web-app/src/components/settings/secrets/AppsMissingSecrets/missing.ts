import type { Secret } from "@/types/secret";
import type { SharedSecretsResponse } from "@/types/sharedSecrets";

/** One key an app lacks, and the workspace secret of that name if there is one. */
export interface MissingKeyRow {
  key: string;
  required: boolean;
  description?: string;
  /** A workspace secret of this name exists: sharing it is the fix. */
  workspaceSecret?: { id: string; appIds: string[] };
}

export interface AppMissing {
  appId: string;
  appName: string;
  keys: MissingKeyRow[];
}

/**
 * Each app still missing a declared key, required keys first, with the matching
 * workspace secret where one exists. Only a plain workspace secret matches — an
 * app's own `apps/<id>/KEY` row is another app's and is never offered.
 */
export function missingByApp(
  sharing: SharedSecretsResponse | undefined,
  secrets: Secret[]
): AppMissing[] {
  if (!sharing) return [];
  const byName = new Map(secrets.filter((s) => !s.name.includes("/")).map((s) => [s.name, s.id]));
  const sharedWith = new Map(sharing.shared.map((s) => [s.secret_id, s.app_ids]));
  return sharing.apps
    .filter((app) => app.missing.length > 0)
    .map((app) => ({
      appId: app.app_id,
      appName: app.app_name,
      keys: [...app.missing]
        .sort((a, b) => Number(b.required) - Number(a.required) || a.key.localeCompare(b.key))
        .map((m) => {
          const id = byName.get(m.key);
          return {
            key: m.key,
            required: m.required,
            description: m.description,
            workspaceSecret: id ? { id, appIds: sharedWith.get(id) ?? [] } : undefined
          };
        })
    }));
}
