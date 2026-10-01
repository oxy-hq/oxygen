import type { AdminOrgMeta, AdminUserRow, AdminWorkspaceRow } from "@/services/api/adminTenants";

/**
 * Derive overview stats from the list endpoints. Local aggregation so the
 * hub renders without a dedicated backend endpoint — the cost is one
 * roundtrip per resource type instead of one, but the lists are paginated
 * (page_size cap 200) and we only need the headline counts plus a few
 * "needs attention" predicates. Replace with a server-side overview
 * aggregate once it matters (`GET /admin/tenants/overview`).
 */

const ONE_DAY_MS = 24 * 60 * 60 * 1000;

const isWithinDays = (iso: string | null | undefined, days: number): boolean => {
  if (!iso) return false;
  const t = new Date(iso).getTime();
  return Date.now() - t <= days * ONE_DAY_MS;
};

const isOlderThanDays = (iso: string | null | undefined, days: number): boolean => {
  if (!iso) return true;
  const t = new Date(iso).getTime();
  return Date.now() - t > days * ONE_DAY_MS;
};

export type TenantStats = {
  total: number;
  /** Resources created in the last 7 days. */
  delta7d: number;
};

export const computeOrgStats = (orgs: AdminOrgMeta[] | undefined): TenantStats => {
  if (!orgs) return { total: 0, delta7d: 0 };
  return {
    total: orgs.length,
    delta7d: orgs.filter((o) => isWithinDays(o.created_at, 7)).length
  };
};

export const computeUserStats = (users: AdminUserRow[] | undefined): TenantStats => {
  if (!users) return { total: 0, delta7d: 0 };
  return {
    total: users.length,
    delta7d: users.filter((u) => isWithinDays(u.created_at, 7)).length
  };
};

export const computeWorkspaceStats = (workspaces: AdminWorkspaceRow[] | undefined): TenantStats => {
  if (!workspaces) return { total: 0, delta7d: 0 };
  return {
    total: workspaces.length,
    delta7d: workspaces.filter((w) => isWithinDays(w.created_at, 7)).length
  };
};

// ── Needs-attention surfaces ────────────────────────────────────────────

export const findStaleOrgs = (orgs: AdminOrgMeta[] | undefined, days = 90): AdminOrgMeta[] => {
  if (!orgs) return [];
  // "Stale" = no member churn since `days` ago, derived here from
  // created_at as a stand-in until updated_at lands on AdminOrgMeta.
  return orgs.filter((o) => isOlderThanDays(o.created_at, days) && o.member_count === 0);
};

export const findOrglessUsers = (users: AdminUserRow[] | undefined): AdminUserRow[] => {
  if (!users) return [];
  return users.filter((u) => u.org_count === 0 && u.status === "active");
};

export const findFailedWorkspaces = (
  workspaces: AdminWorkspaceRow[] | undefined
): AdminWorkspaceRow[] => {
  if (!workspaces) return [];
  return workspaces.filter((w) => w.status === "failed");
};

export const findOrphanWorkspaces = (
  workspaces: AdminWorkspaceRow[] | undefined
): AdminWorkspaceRow[] => {
  if (!workspaces) return [];
  return workspaces.filter((w) => w.org_id === null);
};
