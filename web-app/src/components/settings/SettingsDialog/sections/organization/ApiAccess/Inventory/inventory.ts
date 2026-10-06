import type { TokenOwner } from "@/types/apiToken";
import type { InventoryFilters, InventoryToken } from "@/types/orgApiAccess";
import { type AccessDescription, describeAccess } from "../utils/grants";

export const LEGACY_REVOKE_TOOLTIP = "Legacy API keys can only be revoked by their owner";

/**
 * One inventory read, in its two lists. The org's inventory is the one place a legacy API key and
 * an API token come back together, and they are never shown as one list: tokens first, legacy API
 * keys in a group of their own.
 */
export function splitInventory<T extends Pick<InventoryToken, "kind">>(
  rows: T[]
): { tokens: T[]; legacyKeys: T[] } {
  return {
    tokens: rows.filter((row) => row.kind !== "legacy_key"),
    legacyKeys: rows.filter((row) => row.kind === "legacy_key")
  };
}

/**
 * What an org admin can do about one credential's reach into the org.
 *
 * - `revoke_grant`: a personal token; the org can end its reach here.
 * - `legacy`: a legacy API key; only its owner can revoke it, so the action is
 *   shown disabled with the reason rather than hidden.
 * - `service_account`: owned by one of the org's own accounts; it is revoked
 *   from that account's page, which the row links to.
 * - `none`: nothing to do from here — already revoked, its reach here already ended, or a
 *   sandbox agent token, which its minter and Oxygen staff revoke.
 */
export type InventoryRevoke =
  | { kind: "revoke_grant" }
  | { kind: "legacy"; reason: string }
  | { kind: "service_account"; accountId: string }
  | { kind: "none" };

type RevokeFields = Pick<
  InventoryToken,
  "kind" | "status" | "owner" | "all_access" | "grants_here"
>;

export function inventoryRevokeAction(token: RevokeFields): InventoryRevoke {
  if (token.status === "revoked") return { kind: "none" };

  switch (token.kind) {
    case "legacy_key":
      return { kind: "legacy", reason: LEGACY_REVOKE_TOOLTIP };
    case "service_account":
    case "ci":
      return { kind: "service_account", accountId: token.owner.id };
    case "sandbox_agent":
      // Minted by Oxygen staff for an agent building this org's apps. It is revoked by whoever
      // minted it, or by Oxygen staff; ending its reach from here is not one of the ways.
      return { kind: "none" };
    case "personal": {
      // Ended here: every grant in this org is revoked. That holds for an all-access token too,
      // which the server lists with the revoked org-wide row that blocks it. Nothing lifts a
      // block, so an ended row offers nothing at all, never a "restore".
      const reachEnded =
        token.grants_here.length > 0 && token.grants_here.every((g) => g.revoked_at);
      return reachEnded ? { kind: "none" } : { kind: "revoke_grant" };
    }
  }
}

const OWNER_REACH: AccessDescription = {
  summary: "Everything its owner can reach",
  details: []
};

/**
 * What a credential reaches in this org. A legacy API key, and an all-access token
 * with no narrower grant here, carries no grant to read: it simply inherits
 * its owner's reach, so the cell says that instead of "No access".
 */
export function inventoryAccess(
  token: Pick<InventoryToken, "kind" | "all_access" | "grants_here">
): AccessDescription {
  if (token.kind === "legacy_key") return OWNER_REACH;
  const live = token.grants_here.filter((g) => !g.revoked_at);
  if (live.length === 0 && token.all_access && token.grants_here.length === 0) return OWNER_REACH;
  return describeAccess(token.grants_here);
}

/** Every distinct owner in the list, people first, then by name — for the owner filter. */
export function ownerOptions(tokens: Pick<InventoryToken, "owner">[]): TokenOwner[] {
  const byId = new Map<string, TokenOwner>();
  for (const { owner } of tokens) {
    if (!byId.has(owner.id)) byId.set(owner.id, owner);
  }
  return [...byId.values()].sort((a, b) => {
    if (a.type !== b.type) return a.type === "user" ? -1 : 1;
    return a.label.localeCompare(b.label);
  });
}

export const hasActiveFilters = (filters: InventoryFilters): boolean =>
  !!filters.kind || !!filters.owner || !!filters.workspace_id;

/**
 * Whether the token list is shown at all. Filtered down to legacy API keys it is left out: it
 * could only say "no API tokens match", which is the filter working, not a finding.
 */
export const showsTokenList = (filters: InventoryFilters): boolean => filters.kind !== "legacy_key";

/** Sets or clears one filter. An empty value removes the key, so the query key stays canonical. */
export function withFilter<K extends keyof InventoryFilters>(
  filters: InventoryFilters,
  key: K,
  value: InventoryFilters[K] | undefined
): InventoryFilters {
  const next = { ...filters };
  if (value) next[key] = value;
  else delete next[key];
  return next;
}
