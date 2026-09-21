import { ArrowRight, Building2, Check, Minus, ShieldCheck, TriangleAlert } from "lucide-react";
import { Link } from "react-router-dom";
import { useCompileWorkspaces } from "@/hooks/api/compiles";
import { useFleetHealth } from "@/hooks/api/customApps/useFleetHealth";
import { useQueueStats, useWorkers } from "@/hooks/api/internalJobs";
import useCurrentUser from "@/hooks/api/users/useCurrentUser";
import { useWorkspaceHealth } from "@/hooks/api/workspaceHealth/useWorkspaceHealth";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import { ADMIN_NAV, itemReachable } from "@/pages/admin/AdminLayout/adminNav";
import { AdminEmptyState } from "@/pages/admin/components/AdminEmptyState";
import { AdminPage } from "@/pages/admin/components/AdminPage";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import {
  appsReport,
  compilesReport,
  healthReport,
  jobsReport,
  rankFindings,
  type SourceReport
} from "./findings";

/**
 * `/admin` — what the operator sees on arrival.
 *
 * It used to redirect to Custom apps, so the console had no answer to the question people
 * actually open it with: *does anything need me right now?* Answering it meant visiting
 * Workspace health, Internal jobs, Compiles and Apps in turn and holding four partial
 * pictures in your head. This page asks all four at once and ranks what comes back.
 *
 * Two rules keep it honest:
 *
 * - **Good news is quiet.** A source with nothing wrong collapses to one line. Giving
 *   "all clear" the same weight as an outage is how a status page stops being read.
 * - **Never show a room you cannot enter.** Each source is gated on the same capability
 *   its nav entry declares, so a narrower staff role sees a shorter page rather than a
 *   column of 403s.
 */
/** One page of compile rows. The all-clear sentence says so when the page comes back full. */
const COMPILE_PAGE = 200;

export default function AdminHome() {
  const { data: user } = useCurrentUser();
  const standing = {
    isOwner: user?.is_owner ?? false,
    capabilities: user?.platform_capabilities ?? []
  };
  const can = (to: string) => {
    const item = ADMIN_NAV.find((i) => i.to.split("?")[0] === to);
    return item ? itemReachable(item, standing) : false;
  };

  const canHealth = can(ROUTES.ADMIN.WORKSPACE_HEALTH);
  const canJobs = can(ROUTES.ADMIN.INTERNAL_JOBS);
  const canCompiles = can(ROUTES.ADMIN.COMPILES);
  const canApps = can(ROUTES.ADMIN.CUSTOMER_APPS);
  // The overview reads the org / user / workspace directories, so gate it on the same
  // capability the Organizations rail entry declares.
  const canTenants = can(ROUTES.ADMIN.TENANTS);

  // Fetching is gated on the same standing that decides whether the section renders, not
  // just the result: `/admin` is now the rail logo's target and every stale bookmark's
  // destination, so an App Operator holding only `manage_apps` used to land here and fire
  // four requests the server answers 403.
  //
  // Polling is off where the hook allows it. The detail pages poll because someone is
  // watching them; a landing page that re-fetches every few seconds costs the fleet more
  // than it tells the person about to click through. `useFleetHealth` keeps its own 60s
  // interval — it takes no pause flag — so one of the four does still refresh.
  const health = useWorkspaceHealth({ enabled: canHealth });
  const stats = useQueueStats({ paused: true, enabled: canJobs });
  const workers = useWorkers({ paused: true, enabled: canJobs });
  const compiles = useCompileWorkspaces(
    { limit: COMPILE_PAGE },
    { paused: true, enabled: canCompiles }
  );
  const apps = useFleetHealth(false, { enabled: canApps });

  const reports: SourceReport[] = [
    canHealth ? healthReport(health) : null,
    canJobs ? jobsReport(stats, workers) : null,
    canCompiles ? compilesReport(compiles, COMPILE_PAGE) : null,
    canApps ? appsReport(apps) : null
  ].filter((r): r is SourceReport => r !== null);

  const findings = rankFindings(reports);
  const clear = reports.filter((r) => r.findings.length === 0);
  const unanswered = clear.filter((r) => r.okTone === "unknown").length;
  const loading =
    (canHealth && health.isPending) ||
    (canJobs && (stats.isPending || workers.isPending)) ||
    (canCompiles && compiles.isPending) ||
    (canApps && apps.isPending);

  return (
    <AdminPage
      description={
        loading
          ? "Checking the fleet…"
          : findings.length > 0
            ? `${findings.length} thing${findings.length === 1 ? "" : "s"} need attention, worst first.`
            : unanswered > 0
              ? // "Nothing needs attention" is a claim, and it needs every source to have
                // answered before it can be made.
                `Nothing needs attention among the sources that answered — ${unanswered} could not be checked.`
              : "Nothing needs attention right now."
      }
      data-testid='admin-home'
    >
      {findings.length > 0 ? (
        <ul className='space-y-2' data-testid='admin-home-findings'>
          {findings.map((f) => {
            const tone = ADMIN_TONE[f.tone];
            return (
              <li key={`${f.source.key}-${f.id}`}>
                <Link
                  to={f.to}
                  data-testid={`admin-home-finding-${f.id}`}
                  className={cn(
                    "group flex items-center gap-3 rounded-lg border p-3 transition-colors",
                    tone.bg,
                    "border-transparent ring-1 ring-inset hover:border-foreground/20",
                    tone.ring
                  )}
                >
                  <span className={cn("size-2 shrink-0 rounded-full", tone.dot)} aria-hidden />
                  <span className='min-w-0 flex-1'>
                    <span className={cn("font-medium text-xs", tone.text)}>{f.title}</span>
                    <span className='ml-2 text-muted-foreground text-xs'>{f.detail}</span>
                  </span>
                  <span className='shrink-0 text-[10px] text-muted-foreground uppercase tracking-[0.14em]'>
                    {f.source.label}
                  </span>
                  <ArrowRight className='size-3 shrink-0 text-muted-foreground transition-transform group-hover:translate-x-0.5' />
                </Link>
              </li>
            );
          })}
        </ul>
      ) : null}

      {reports.length === 0 ? (
        // A staff grant that reaches none of the four operational surfaces. Rare, but the
        // alternative is a blank page that looks broken rather than correctly narrow.
        <AdminEmptyState
          icon={ShieldCheck}
          title='Nothing to monitor with this grant.'
          description='Your platform grant does not reach workspace health, internal jobs, compiles or custom apps. The rail shows everything you can open.'
        />
      ) : null}

      {clear.length > 0 ? (
        <section data-testid='admin-home-clear' className='space-y-1'>
          {findings.length > 0 ? (
            <p className='font-medium text-[10px] text-muted-foreground uppercase tracking-[0.14em]'>
              Otherwise clear
            </p>
          ) : null}
          <ul className='divide-y divide-border/60 overflow-hidden rounded-lg border border-border/60'>
            {clear.map((r) => (
              <li key={r.key}>
                <Link
                  to={r.to}
                  data-testid={`admin-home-clear-${r.key}`}
                  className='flex items-center gap-2.5 px-3 py-2 transition-colors hover:bg-muted/40'
                >
                  {r.okTone === "unknown" ? (
                    <TriangleAlert
                      className={cn("size-3 shrink-0", ADMIN_TONE.warn.text)}
                      aria-hidden
                    />
                  ) : r.okTone === "muted" ? (
                    <Minus className={cn("size-3 shrink-0", ADMIN_TONE.muted.text)} aria-hidden />
                  ) : (
                    <Check className={cn("size-3 shrink-0", ADMIN_TONE.ok.text)} aria-hidden />
                  )}
                  <span className='w-40 shrink-0 font-medium text-xs'>{r.label}</span>
                  <span className='min-w-0 flex-1 truncate text-muted-foreground text-xs'>
                    {loading ? "Checking…" : r.ok}
                  </span>
                </Link>
              </li>
            ))}
          </ul>
        </section>
      ) : null}
      {canTenants ? (
        <Link
          to={ROUTES.ADMIN.TENANTS_OVERVIEW}
          data-testid='admin-home-tenants-overview'
          className='flex items-center gap-2 text-muted-foreground text-xs transition-colors hover:text-foreground'
        >
          <Building2 className='size-3' />
          Tenant-side triage: stale orgs, users with no org, orphan workspaces
          <ArrowRight className='size-3' />
        </Link>
      ) : null}
    </AdminPage>
  );
}
