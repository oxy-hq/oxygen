import { Building2, FolderOpen, Users } from "lucide-react";
import { useMemo } from "react";
import { useAdminOrgsList } from "@/hooks/api/adminTenants/useAdminOrgs";
import { useAdminUsersList } from "@/hooks/api/adminTenants/useAdminUsers";
import { useAdminWorkspacesList } from "@/hooks/api/adminTenants/useAdminWorkspaces";
import ROUTES from "@/libs/utils/routes";
import { AdminPage } from "../components/AdminPage";
import { AdminStatCard } from "../components/AdminStatCard";
import { LlmCostSection } from "./components/LlmCostSection";
import { ATTENTION_ICONS, attentionLink, NeedsAttention } from "./components/NeedsAttention";
import { ResourceRoster } from "./components/ResourceRoster";
import {
  computeOrgStats,
  computeUserStats,
  computeWorkspaceStats,
  findFailedWorkspaces,
  findOrglessUsers,
  findOrphanWorkspaces,
  findStaleOrgs
} from "./utils";

/**
 * `/admin/tenants` — Operator overview hub.
 *
 * Single landing surface for Global Owners and Global Admins doing tenant
 * triage. Replaces the implicit "I have to click into each list to know
 * if anything's wrong" automation with an at-a-glance summary:
 *
 *   - Headline counts for orgs / users / workspaces, with a 7-day delta
 *     so growth is visible even before the operator opens a chart.
 *   - "Needs attention" feed surfaces three cross-tenant anomalies
 *     derived from the list endpoints: stale orgs, users without any
 *     org, failed workspaces, orphan workspaces (no org). Each row
 *     deep-links into the focused list.
 *   - Recently created roster per resource — quick smell test for what
 *     joined the fleet today.
 *
 * Information density is intentionally restrained: this page tells the
 * operator whether to dig deeper, not what to do once they get there.
 */
export default function AdminTenants() {
  const orgs = useAdminOrgsList({ page_size: 200 });
  const users = useAdminUsersList({ page_size: 200 });
  const workspaces = useAdminWorkspacesList({ page_size: 200 });

  const orgStats = useMemo(() => computeOrgStats(orgs.data), [orgs.data]);
  const userStats = useMemo(() => computeUserStats(users.data), [users.data]);
  const workspaceStats = useMemo(() => computeWorkspaceStats(workspaces.data), [workspaces.data]);

  const stale = useMemo(() => findStaleOrgs(orgs.data), [orgs.data]);
  const orgless = useMemo(() => findOrglessUsers(users.data), [users.data]);
  const failed = useMemo(() => findFailedWorkspaces(workspaces.data), [workspaces.data]);
  const orphan = useMemo(() => findOrphanWorkspaces(workspaces.data), [workspaces.data]);

  // Sort by created_at desc to match the "Recently created" framing — the
  // list endpoints return rows in their own default order (alphabetical for
  // orgs, last-login desc for users), neither of which is creation recency.
  const recentOrgs = useMemo(
    () =>
      [...(orgs.data ?? [])]
        .sort((a, b) => new Date(b.created_at).getTime() - new Date(a.created_at).getTime())
        .slice(0, 4),
    [orgs.data]
  );
  const recentUsers = useMemo(
    () =>
      [...(users.data ?? [])]
        .sort((a, b) => new Date(b.created_at).getTime() - new Date(a.created_at).getTime())
        .slice(0, 4),
    [users.data]
  );
  const recentWorkspaces = useMemo(
    () =>
      [...(workspaces.data ?? [])]
        .sort((a, b) => new Date(b.created_at).getTime() - new Date(a.created_at).getTime())
        .slice(0, 4),
    [workspaces.data]
  );

  const isPending = orgs.isPending || users.isPending || workspaces.isPending;

  return (
    <AdminPage
      description='Cross-tenant triage: what grew, what is costing, and what is stuck. The console home covers the operational side.'
      bodyClassName='space-y-8'
      data-testid='admin-tenants-overview'
    >
      {/* ── Headline stats ──────────────────────────────────────────── */}
      <section aria-label='Headline counts' className='grid gap-4 md:grid-cols-3'>
        <AdminStatCard
          label='Organizations'
          value={isPending ? "—" : orgStats.total.toLocaleString()}
          delta={isPending ? undefined : { value: orgStats.delta7d, suffix: "7d" }}
          icon={Building2}
          to={ROUTES.ADMIN.ORGS}
        />
        <AdminStatCard
          label='Users'
          value={isPending ? "—" : userStats.total.toLocaleString()}
          delta={isPending ? undefined : { value: userStats.delta7d, suffix: "7d" }}
          icon={Users}
          to={ROUTES.ADMIN.USERS}
        />
        <AdminStatCard
          label='Workspaces'
          value={isPending ? "—" : workspaceStats.total.toLocaleString()}
          delta={isPending ? undefined : { value: workspaceStats.delta7d, suffix: "7d" }}
          icon={FolderOpen}
          to={ROUTES.ADMIN.WORKSPACES}
        />
      </section>

      {/* ── LLM cost & usage ────────────────────────────────────────── */}
      <LlmCostSection />

      {/* ── Needs attention ─────────────────────────────────────────── */}
      <section aria-label='Needs attention'>
        <NeedsAttention
          isLoading={isPending}
          rows={[
            {
              icon: ATTENTION_ICONS.org,
              count: stale.length,
              label: "orgs with no members and no recent activity",
              to: attentionLink.staleOrgs,
              severity: "warn"
            },
            {
              icon: ATTENTION_ICONS.user,
              count: orgless.length,
              label: "active users without any org membership",
              to: attentionLink.orglessUsers,
              severity: "warn"
            },
            {
              icon: ATTENTION_ICONS.workspace,
              count: failed.length,
              label: "workspaces in failed state",
              to: attentionLink.failedWorkspaces,
              severity: "danger"
            },
            {
              icon: ATTENTION_ICONS.workspace,
              count: orphan.length,
              label: "workspaces with no parent org",
              to: attentionLink.orphanWorkspaces,
              severity: "warn"
            }
          ]}
        />
      </section>

      {/* ── Recently created rosters ────────────────────────────────── */}
      <section aria-label='Recently created' className='grid gap-6 md:grid-cols-2 xl:grid-cols-3'>
        <ResourceRoster
          label='Recently created · orgs'
          isLoading={isPending}
          seeAllHref={ROUTES.ADMIN.ORGS}
          emptyLabel='No organizations on this deployment yet.'
          rows={recentOrgs.map((o) => ({
            primary: o.name,
            secondary: o.slug,
            trailing: `${o.member_count} members`,
            to: ROUTES.ADMIN.ORG_DETAIL(o.id)
          }))}
        />
        <ResourceRoster
          label='Recently joined · users'
          isLoading={isPending}
          seeAllHref={ROUTES.ADMIN.USERS}
          emptyLabel='No users on this deployment yet.'
          rows={recentUsers.map((u) => ({
            primary: u.name || u.email,
            secondary: u.email,
            trailing: `${u.org_count} org${u.org_count === 1 ? "" : "s"}`,
            to: ROUTES.ADMIN.USER_DETAIL(u.id)
          }))}
        />
        <ResourceRoster
          label='Recently created · workspaces'
          isLoading={isPending}
          seeAllHref={ROUTES.ADMIN.WORKSPACES}
          emptyLabel='No workspaces on this deployment yet.'
          rows={recentWorkspaces.map((w) => ({
            primary: w.name,
            secondary: w.org_slug ?? "no org",
            trailing: `${w.member_count} members`,
            to: ROUTES.ADMIN.WORKSPACE_DETAIL(w.id)
          }))}
        />
      </section>
    </AdminPage>
  );
}
