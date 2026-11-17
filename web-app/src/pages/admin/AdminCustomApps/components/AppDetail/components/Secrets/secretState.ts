import type { AppSecretEntry } from "@/types/apps";

/**
 * Where a key with no value of its own reads one from, or `null` when it reads
 * nothing. One rule for the row's badge and the pane's "Unset only" filter, so the
 * filter can never hide a row the list calls unset, or show one it calls inherited.
 *
 * `productionHolds` is whether production stores the key, from production's own
 * view; `undefined` while that is unknown, which is read as "it might" so a shared
 * key is not called missing before the answer arrives.
 */
export function inheritsFrom(
  entry: AppSecretEntry,
  productionHolds: boolean | undefined
): "staging" | "production" | "workspace" | null {
  if (entry.is_set) return null;
  if (entry.shared_from_workspace) return "workspace";
  if (entry.inherits_staging) return "staging";
  if (entry.inherits_production && productionHolds !== false) return "production";
  return null;
}

/** No value here and none to read from elsewhere: someone has to set it. */
export function isUnset(entry: AppSecretEntry, productionHolds: boolean | undefined): boolean {
  return !entry.is_set && inheritsFrom(entry, productionHolds) === null;
}
