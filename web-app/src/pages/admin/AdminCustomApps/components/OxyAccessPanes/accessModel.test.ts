import { describe, expect, it } from "vitest";
import type { AdminOrgMeta } from "@/services/api/adminTenants";
import type { OxyAccessRow } from "@/types/apps";
import { buildAccessOrgs } from "./accessModel";

const org = (over: Partial<AdminOrgMeta>): AdminOrgMeta => ({
  id: "o",
  name: "Org",
  slug: "org",
  created_at: "2026-01-01T00:00:00Z",
  member_count: 0,
  workspace_count: 0,
  owner_email: null,
  partner: null,
  is_partner: false,
  ...over
});

const row = (over: Partial<OxyAccessRow>): OxyAccessRow => ({
  workspace_id: "w",
  workspace_name: "WS",
  org_id: "o",
  org_name: "Org",
  org_slug: "org",
  accessible: true,
  locked: false,
  locked_by_email: null,
  locked_at: null,
  ...over
});

describe("buildAccessOrgs", () => {
  it("counts open vs locked workspaces per org", () => {
    const orgs = [org({ id: "a", name: "Alpha", slug: "alpha", workspace_count: 2 })];
    const rows = [
      row({ org_id: "a", org_slug: "alpha", org_name: "Alpha", workspace_id: "w1" }),
      row({
        org_id: "a",
        org_slug: "alpha",
        org_name: "Alpha",
        workspace_id: "w2",
        accessible: false,
        locked: true,
        locked_at: "2026-07-14T00:00:00Z",
        locked_by_email: "owner@alpha.test"
      })
    ];

    const [a] = buildAccessOrgs(orgs, rows);
    expect(a.workspaces).toHaveLength(2);
    expect(a.accessibleCount).toBe(1);
    expect(a.lockedCount).toBe(1);
    expect(a.hasLockdown).toBe(true);
  });

  it("an org with no lockdown is fully open — staff access is the default", () => {
    const orgs = [org({ id: "b", name: "Beta", slug: "beta", workspace_count: 1 })];
    const rows = [row({ org_id: "b", org_slug: "beta", org_name: "Beta", workspace_id: "w1" })];

    const [b] = buildAccessOrgs(orgs, rows);
    expect(b.hasLockdown).toBe(false);
    expect(b.lockedCount).toBe(0);
    expect(b.accessibleCount).toBe(1);
  });

  it("sorts locked-out orgs first, then by name", () => {
    const orgs = [
      org({ id: "a", name: "Alpha", slug: "alpha" }),
      org({ id: "b", name: "Beta", slug: "beta" }),
      org({ id: "c", name: "Gamma", slug: "gamma" })
    ];
    const rows = [
      row({ org_id: "a", org_slug: "alpha", org_name: "Alpha", workspace_id: "w1" }),
      // Gamma has a lockdown → must sort ahead of Alpha/Beta.
      row({
        org_id: "c",
        org_slug: "gamma",
        org_name: "Gamma",
        workspace_id: "w2",
        accessible: false,
        locked: true
      })
    ];

    expect(buildAccessOrgs(orgs, rows).map((o) => o.orgId)).toEqual(["c", "a", "b"]);
  });

  it("unions in an org missing from the admin directory so a lockdown is never hidden", () => {
    const result = buildAccessOrgs(
      [],
      [row({ org_id: "x", org_slug: "x", org_name: "X", accessible: false, locked: true })]
    );
    expect(result).toHaveLength(1);
    expect(result[0].hasLockdown).toBe(true);
    expect(result[0].workspaceCount).toBe(0);
  });

  it("returns an empty list when there are no orgs or rows", () => {
    expect(buildAccessOrgs([], [])).toEqual([]);
  });
});
