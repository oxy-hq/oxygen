import { AlertTriangle, FileCheck, FolderOpen, Play, Users } from "lucide-react";
import { Link } from "react-router-dom";
import { useExplorerRuns } from "@/hooks/api/adminExplorer";
import { useCompiles } from "@/hooks/api/compiles";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import { ago } from "@/pages/admin/AdminExplorer/format";
import type { ExplorerRun } from "@/services/api/adminExplorer";
import type { OrgUsageDetail } from "@/services/api/adminMetrics";
import type { AdminOrgDetail } from "@/services/api/adminTenants";
import { AdminAsync } from "../../../components/AdminAsync";
import { AdminEmptyState } from "../../../components/AdminEmptyState";
import { AdminLinkedList, AdminLinkedRow } from "../../../components/AdminLinkedRow";
import { AdminSectionLabel } from "../../../components/AdminSectionLabel";
import type { OrgTabId } from "../tabs";
import { NeedsAttention, type OrgAlert } from "./NeedsAttention";
import { OrgCostCard } from "./OrgCostCard";
import { RoleBadge, WorkspaceStatusPill } from "./StatusBadges";

const RECENT_LIMIT = 6;
const VIEW_ALL_CLASS = "font-medium text-[10px] uppercase tracking-[0.14em] hover:text-foreground";

/**
 * The org-360 landing tab — the triage screen. Pulls together the small set of
 * signals an operator wants first: anomalies that need attention, the latest
 * activity, the org's workspaces and members, and a cost snapshot. Each card
 * has a "view all" that drills into the dedicated tab.
 */
export const OrgOverviewTab = ({
  detail,
  usage,
  usageLoading,
  usageDays,
  onSelectTab
}: {
  detail: AdminOrgDetail;
  usage: OrgUsageDetail | null | undefined;
  usageLoading: boolean;
  usageDays: number;
  onSelectTab: (tab: OrgTabId) => void;
}) => {
  const recentRuns = useExplorerRuns({ orgId: detail.id, pageSize: RECENT_LIMIT });
  const compiles = useCompiles({ org_id: detail.id, limit: 100 }, { paused: true });
  const runs = recentRuns.data?.items ?? [];
  const erroredRuns = runs.filter(
    (r) => r.task_status === "failed" || r.task_status === "dead"
  ).length;
  const failedWorkspaces = detail.workspaces.filter((w) => w.status === "failed").length;
  const failedCompiles = (compiles.data?.rows ?? []).filter(
    (r) => r.status === "failed" && r.is_current_for_workspace
  ).length;

  const alerts: OrgAlert[] = [];
  if (failedWorkspaces > 0)
    alerts.push({
      icon: FolderOpen,
      text: `${failedWorkspaces} workspace${failedWorkspaces === 1 ? "" : "s"} failed to clone or compile`,
      severity: "danger",
      onSelect: () => onSelectTab("workspaces")
    });
  if (failedCompiles > 0)
    alerts.push({
      icon: FileCheck,
      text: `${failedCompiles} workspace${failedCompiles === 1 ? "" : "s"} have a failing current compile`,
      severity: "danger",
      onSelect: () => onSelectTab("compiles")
    });
  if (erroredRuns > 0)
    alerts.push({
      icon: AlertTriangle,
      text: `${erroredRuns} recent run${erroredRuns === 1 ? "" : "s"} errored`,
      severity: "danger",
      onSelect: () => onSelectTab("activity")
    });
  if (detail.member_count === 0)
    alerts.push({
      icon: Users,
      text: "Organization has no members",
      severity: "warn",
      onSelect: () => onSelectTab("members")
    });

  return (
    <div className='space-y-6'>
      <NeedsAttention alerts={alerts} />

      <div className='grid gap-6 lg:grid-cols-2'>
        <section className='space-y-3'>
          <AdminSectionLabel
            trailing={
              <button
                type='button'
                onClick={() => onSelectTab("activity")}
                className={VIEW_ALL_CLASS}
              >
                View all →
              </button>
            }
          >
            Recent activity
          </AdminSectionLabel>
          {/* The failed state used to fall through to "No runs yet" — a triage screen
              claiming the tenant is idle when it simply could not ask. */}
          <AdminAsync
            query={recentRuns}
            noun='recent activity'
            skeleton={<PreviewSkeleton />}
            isEmpty={(d) => d.items.length === 0}
            empty={
              <AdminEmptyState
                icon={Play}
                title='No runs yet'
                description='Agent runs for this organization appear here.'
              />
            }
          >
            {(d) => (
              <ul className='divide-y divide-border/60 overflow-hidden rounded-md border border-border/60 bg-card'>
                {d.items.map((r) => (
                  <RecentRunRow key={r.id} run={r} />
                ))}
              </ul>
            )}
          </AdminAsync>
        </section>

        <section className='space-y-3'>
          <AdminSectionLabel
            trailing={
              detail.workspaces.length > 4 ? (
                <button
                  type='button'
                  onClick={() => onSelectTab("workspaces")}
                  className={VIEW_ALL_CLASS}
                >
                  View all →
                </button>
              ) : null
            }
          >
            Recent workspaces
          </AdminSectionLabel>
          {detail.workspaces.length === 0 ? (
            <AdminEmptyState
              icon={FolderOpen}
              title='No workspaces yet'
              description='Workspaces appear here when members import a repository.'
            />
          ) : (
            <AdminLinkedList>
              {detail.workspaces.slice(0, 4).map((w) => (
                <AdminLinkedRow
                  key={w.id}
                  to={ROUTES.ADMIN.WORKSPACE_DETAIL(w.id)}
                  icon={FolderOpen}
                  primary={w.name}
                  secondary={`Created ${new Date(w.created_at).toLocaleDateString()}`}
                  meta={<WorkspaceStatusPill status={w.status} />}
                />
              ))}
            </AdminLinkedList>
          )}
        </section>
      </div>

      <div className='grid gap-6 lg:grid-cols-2'>
        <section className='space-y-3'>
          <AdminSectionLabel
            trailing={
              detail.owners.length > 4 ? (
                <button
                  type='button'
                  onClick={() => onSelectTab("members")}
                  className={VIEW_ALL_CLASS}
                >
                  View all →
                </button>
              ) : null
            }
          >
            Top members
          </AdminSectionLabel>
          {detail.owners.length === 0 ? (
            <AdminEmptyState
              icon={Users}
              title='No members yet'
              description='Add members via the organization settings page.'
            />
          ) : (
            <AdminLinkedList>
              {detail.owners.slice(0, 4).map((m) => (
                <AdminLinkedRow
                  key={m.user_id}
                  to={ROUTES.ADMIN.USER_DETAIL(m.user_id)}
                  icon={Users}
                  primary={m.name || m.email}
                  secondary={m.email}
                  meta={<RoleBadge role={m.role} />}
                />
              ))}
            </AdminLinkedList>
          )}
        </section>

        <OrgCostCard usage={usage} days={usageDays} isLoading={usageLoading} />
      </div>
    </div>
  );
};

const PreviewSkeleton = () => (
  <div className='space-y-1.5'>
    {[0, 1, 2].map((i) => (
      <div key={i} className='h-11 animate-pulse rounded-md bg-muted/40' />
    ))}
  </div>
);

const STATUS_DOT: Record<string, string> = {
  failed: "bg-destructive",
  dead: "bg-destructive",
  running: "bg-primary",
  delegating: "bg-primary",
  awaiting_input: "bg-primary",
  done: "bg-success",
  cancelled: "bg-muted-foreground/60"
};

const RecentRunRow = ({ run }: { run: ExplorerRun }) => {
  const status = run.task_status ?? "unknown";
  const dot = STATUS_DOT[status] ?? "bg-foreground/40";
  const openable = run.org_slug && run.workspace_id && run.thread_id;
  const inner = (
    <div className='flex items-center gap-2.5 px-3 py-2.5 text-xs'>
      <span className={cn("size-1.5 shrink-0 rounded-full", dot)} aria-hidden />
      <span className='min-w-0 flex-1 truncate'>{run.question_snippet || "(no question)"}</span>
      <span className='shrink-0 text-muted-foreground tabular-nums'>{ago(run.created_at)}</span>
    </div>
  );
  if (openable) {
    return (
      <li>
        <Link
          to={ROUTES.ORG(run.org_slug as string)
            .WORKSPACE(run.workspace_id as string)
            .THREAD(run.thread_id as string)}
          className='block transition-colors hover:bg-muted/40'
        >
          {inner}
        </Link>
      </li>
    );
  }
  return <li>{inner}</li>;
};
