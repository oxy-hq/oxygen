import { useQueryClient } from "@tanstack/react-query";
import { HeartPulse, RefreshCw } from "lucide-react";
import { useSearchParams } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import queryKeys from "@/hooks/api/queryKey";
import { useWorkspaceHealth } from "@/hooks/api/workspaceHealth/useWorkspaceHealth";
import { cn } from "@/libs/shadcn/utils";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import { AdminEmptyState } from "@/pages/admin/components/AdminEmptyState";
import { AdminPage } from "@/pages/admin/components/AdminPage";
import { CauseCard } from "./components/CauseCard";
import { WorkspaceTable } from "./components/WorkspaceTable";
import { groupByCause } from "./groupByCause";

/**
 * Cross-tenant workspace health rollup, from `GET /admin/workspace-health`.
 *
 * **Grouped by cause, not by workspace.** A platform breaks in platform-shaped ways: one
 * refused connection or one unconfigured connector lands on every workspace that touches
 * it. Listed per workspace, the seeded fleet rendered as nine rows carrying nine copies
 * of the same 206-character failure, each truncated at the viewport edge — nine apparent
 * incidents, one real one, and no way to tell which from the screen.
 *
 * The per-workspace table is still here behind `?view=workspaces`, because "is this one
 * workspace ok?" is a different question and the answer belongs on the same page.
 */
export default function AdminWorkspaceHealthPage() {
  const qc = useQueryClient();
  const health = useWorkspaceHealth();
  const [params, setParams] = useSearchParams();
  // View lives in the URL so an operator can paste "the cause view of the fleet" into
  // Slack and have it open that way.
  const view = params.get("view") === "workspaces" ? "workspaces" : "causes";

  const setView = (next: "causes" | "workspaces") => {
    const p = new URLSearchParams(params);
    if (next === "causes") p.delete("view");
    else p.set("view", next);
    setParams(p, { replace: true });
  };

  return (
    <AdminPage
      data-testid='admin-workspace-health-page'
      actions={
        <>
          <div className='flex items-center rounded-md border border-border/60 p-0.5'>
            {(["causes", "workspaces"] as const).map((v) => (
              <button
                key={v}
                type='button'
                onClick={() => setView(v)}
                data-testid={`admin-workspace-health-view-${v}`}
                aria-pressed={view === v}
                className={cn(
                  "rounded-sm px-2 py-1 text-xs capitalize transition-colors",
                  view === v
                    ? "bg-muted font-medium text-foreground"
                    : "text-muted-foreground hover:text-foreground"
                )}
              >
                By {v === "causes" ? "cause" : "workspace"}
              </button>
            ))}
          </div>
          <Button
            variant='outline'
            size='sm'
            onClick={() => qc.invalidateQueries({ queryKey: queryKeys.workspaceHealth.all })}
            disabled={health.isFetching}
            className='gap-1.5'
          >
            <RefreshCw className={health.isFetching ? "animate-spin" : ""} aria-hidden />
            Refresh
          </Button>
        </>
      }
    >
      <AdminAsync
        query={health}
        noun='workspace health'
        rows={4}
        isEmpty={(d) => d.workspaces.length === 0}
        empty={
          <AdminEmptyState
            icon={HeartPulse}
            title='No workspaces are being evaluated.'
            description='Health checks are opt-in: a workspace is evaluated only once its config.yml carries a health_check: block.'
          />
        }
      >
        {(data) => {
          const causes = groupByCause(data.workspaces);
          const unhealthy = data.workspaces.filter((w) => w.status !== "healthy").length;
          const healthy = data.workspaces.length - unhealthy;

          return (
            <>
              {/* The headline the old page never stated: how many incidents, not how
                  many rows. Nine rows reading as nine problems was the whole defect. */}
              <p
                data-testid='admin-workspace-health-summary'
                className='text-muted-foreground text-xs'
              >
                <span className='font-medium text-foreground tabular-nums'>{unhealthy}</span> of{" "}
                <span className='tabular-nums'>{data.workspaces.length}</span> workspaces need
                attention
                {causes.length > 0 ? (
                  <>
                    {" — from "}
                    <span className='font-medium text-foreground tabular-nums'>
                      {causes.length}
                    </span>{" "}
                    distinct cause{causes.length === 1 ? "" : "s"}
                  </>
                ) : null}
                {healthy > 0 ? (
                  <span className='tabular-nums'>
                    {" · "}
                    {healthy} healthy
                  </span>
                ) : null}
              </p>

              {view === "workspaces" ? (
                <WorkspaceTable workspaces={data.workspaces} />
              ) : causes.length === 0 ? (
                <AdminEmptyState
                  icon={HeartPulse}
                  title='Every evaluated workspace is healthy.'
                  description='Nothing is failing across the fleet right now.'
                />
              ) : (
                <div className='space-y-3'>
                  {causes.map((cause, i) => (
                    <CauseCard key={cause.id} cause={cause} index={i} />
                  ))}
                </div>
              )}
            </>
          );
        }}
      </AdminAsync>
    </AdminPage>
  );
}
