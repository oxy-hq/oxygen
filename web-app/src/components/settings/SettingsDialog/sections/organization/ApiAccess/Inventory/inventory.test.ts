import { describe, expect, it } from "vitest";
import type { Grant } from "@/types/apiToken";
import type { InventoryToken } from "@/types/orgApiAccess";
import {
  hasActiveFilters,
  inventoryAccess,
  inventoryRevokeAction,
  LEGACY_REVOKE_TOOLTIP,
  ownerOptions,
  showsTokenList,
  splitInventory,
  withFilter
} from "./inventory";

const grant = (over: Partial<Grant> = {}): Grant => ({
  id: "g1",
  kind: "workspace",
  org_id: "org-1",
  org_name: "Acme",
  workspace_id: "ws-1",
  workspace_name: "Analytics",
  role_ceiling: "member",
  app_id: null,
  app_name: null,
  revoked_at: null,
  ...over
});

type Row = Pick<InventoryToken, "kind" | "status" | "owner" | "all_access" | "grants_here">;

const row = (over: Partial<Row> = {}): Row => ({
  kind: "personal",
  status: "active",
  owner: { type: "user", id: "u-1", label: "Ada Lovelace" },
  all_access: false,
  grants_here: [grant()],
  ...over
});

describe("splitInventory", () => {
  it("keeps legacy API keys out of the token list, and tokens out of the legacy list", () => {
    const rows = (["personal", "legacy_key", "service_account", "legacy_key", "ci"] as const).map(
      (kind) => row({ kind })
    );
    const { tokens, legacyKeys } = splitInventory(rows);
    expect(tokens.map((t) => t.kind)).toEqual(["personal", "service_account", "ci"]);
    expect(legacyKeys.map((k) => k.kind)).toEqual(["legacy_key", "legacy_key"]);
  });

  it("gives two empty lists for an empty read", () => {
    expect(splitInventory([])).toEqual({ tokens: [], legacyKeys: [] });
  });

  it("drops the token list only when the filter asks for legacy API keys", () => {
    expect(showsTokenList({})).toBe(true);
    expect(showsTokenList({ kind: "personal", owner: "u-1" })).toBe(true);
    expect(showsTokenList({ kind: "legacy_key" })).toBe(false);
  });
});

describe("inventoryRevokeAction", () => {
  it("lets the org end a personal token's reach", () => {
    expect(inventoryRevokeAction(row())).toEqual({ kind: "revoke_grant" });
  });

  it("covers an all-access personal token, which has no grant to point at", () => {
    expect(inventoryRevokeAction(row({ all_access: true, grants_here: [] }))).toEqual({
      kind: "revoke_grant"
    });
  });

  it("shows a legacy API key's action disabled, with the reason", () => {
    expect(inventoryRevokeAction(row({ kind: "legacy_key", all_access: true }))).toEqual({
      kind: "legacy",
      reason: LEGACY_REVOKE_TOOLTIP
    });
    expect(LEGACY_REVOKE_TOOLTIP).toBe("Legacy API keys can only be revoked by their owner");
  });

  it.each(["service_account", "ci"] as const)("sends a %s token to its account's page", (kind) => {
    const owner = { type: "service_account" as const, id: "sa-1", label: "deployer" };
    expect(inventoryRevokeAction(row({ kind, owner }))).toEqual({
      kind: "service_account",
      accountId: "sa-1"
    });
  });

  it("offers nothing on a revoked token, whatever its kind", () => {
    for (const kind of ["personal", "legacy_key", "service_account", "ci"] as const) {
      expect(inventoryRevokeAction(row({ kind, status: "revoked" }))).toEqual({ kind: "none" });
    }
  });

  it("offers nothing once the org has already ended a personal token's reach", () => {
    const ended = row({ grants_here: [grant({ revoked_at: "2026-09-01T00:00:00Z" })] });
    expect(inventoryRevokeAction(ended)).toEqual({ kind: "none" });
  });

  it("offers nothing on a blocked all-access token, and never a way to restore it", () => {
    // The server lists it with the revoked org-wide row that blocks it; no route lifts one.
    const blocked = row({
      all_access: true,
      grants_here: [grant({ workspace_id: null, revoked_at: "2026-09-01T00:00:00Z" })]
    });
    expect(inventoryRevokeAction(blocked)).toEqual({ kind: "none" });
  });

  it("still offers it on an expired personal token, which its owner could extend", () => {
    expect(inventoryRevokeAction(row({ status: "expired" }))).toEqual({ kind: "revoke_grant" });
  });
});

describe("inventoryAccess", () => {
  it("describes the grants a token holds here", () => {
    expect(inventoryAccess(row()).summary).toBe("Analytics as Member");
  });

  it("says a legacy key inherits its owner's reach", () => {
    expect(inventoryAccess(row({ kind: "legacy_key", grants_here: [] })).summary).toBe(
      "Everything its owner can reach"
    );
  });

  it("says the same of an all-access token with no narrower grant here", () => {
    expect(inventoryAccess(row({ all_access: true, grants_here: [] })).summary).toBe(
      "Everything its owner can reach"
    );
  });

  it("prefers the grants when an all-access token has some here", () => {
    expect(inventoryAccess(row({ all_access: true })).summary).toBe("Analytics as Member");
  });

  it("says so when the org revoked its reach", () => {
    const revoked = row({ grants_here: [grant({ revoked_at: "2026-09-01T00:00:00Z" })] });
    expect(inventoryAccess(revoked).summary).toBe("Access revoked");
  });
});

describe("ownerOptions", () => {
  it("lists each owner once, people before service accounts, each group by name", () => {
    const owners = ownerOptions([
      { owner: { type: "service_account", id: "sa-1", label: "deployer" } },
      { owner: { type: "user", id: "u-2", label: "Grace Hopper" } },
      { owner: { type: "user", id: "u-1", label: "Ada Lovelace" } },
      { owner: { type: "user", id: "u-2", label: "Grace Hopper" } },
      { owner: { type: "service_account", id: "sa-0", label: "ci-bot" } }
    ]);
    expect(owners.map((o) => o.label)).toEqual([
      "Ada Lovelace",
      "Grace Hopper",
      "ci-bot",
      "deployer"
    ]);
  });

  it("is empty for an empty list", () => {
    expect(ownerOptions([])).toEqual([]);
  });
});

describe("filters", () => {
  it("sets a filter", () => {
    expect(withFilter({}, "kind", "personal")).toEqual({ kind: "personal" });
  });

  it("removes the key when cleared, instead of leaving it undefined", () => {
    const cleared = withFilter({ kind: "personal", owner: "u-1" }, "kind", undefined);
    expect(cleared).toEqual({ owner: "u-1" });
    expect(cleared).not.toHaveProperty("kind");
  });

  it("does not mutate the filters it was given", () => {
    const filters = { kind: "personal" as const };
    withFilter(filters, "owner", "u-1");
    expect(filters).toEqual({ kind: "personal" });
  });

  it("knows whether any filter is on", () => {
    expect(hasActiveFilters({})).toBe(false);
    expect(hasActiveFilters({ workspace_id: "ws-1" })).toBe(true);
  });
});
