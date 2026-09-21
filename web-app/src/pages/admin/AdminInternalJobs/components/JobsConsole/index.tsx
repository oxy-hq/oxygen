import { useQueryClient } from "@tanstack/react-query";
import { CheckCircle2, Search } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { toast } from "sonner";
import { Checkbox } from "@/components/ui/shadcn/checkbox";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { useDeleteDead, useRecentFailures, useReenqueueDead } from "@/hooks/api/internalJobs";
import queryKeys from "@/hooks/api/queryKey";
import { cn } from "@/libs/utils/cn";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import { AdminEmptyState } from "@/pages/admin/components/AdminEmptyState";
import { InternalJobsService } from "@/services/api/internalJobs";
import { useLive } from "../../LiveContext";
import { BulkActionBar } from "./BulkActionBar";
import { JobRow } from "./JobRow";

const LIMIT = 200;
type Filter = "all" | "failed" | "dead";
const FILTERS: { id: Filter; label: string }[] = [
  { id: "all", label: "All" },
  { id: "failed", label: "Failed" },
  { id: "dead", label: "Dead" }
];

/**
 * The console centerpiece: a dense, expandable table of failed + dead jobs,
 * each row drillable into a full debug panel (error, decoded spec, the
 * workspace / org / user it belongs to). This is the primary surface — the
 * charts above it are at-a-glance context, this is where the operator works.
 *
 * Selection + bulk retry/delete apply to `dead` rows only. Bulk paths call
 * the service directly so the per-mutation toast/invalidate from the single
 * hooks don't fire N times — one summary toast, one invalidation.
 */
export const JobsConsole = () => {
  const { paused } = useLive();
  const qc = useQueryClient();
  const failures = useRecentFailures(LIMIT, { paused });
  const data = failures.data;
  const reenqueue = useReenqueueDead();
  const remove = useDeleteDead();

  const [filter, setFilter] = useState<Filter>("all");
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [isBulking, setIsBulking] = useState<"reenqueue" | "delete" | null>(null);

  const rows = useMemo(() => {
    let all = data ?? [];
    if (filter !== "all") all = all.filter((r) => r.queue_status === filter);
    const q = query.trim().toLowerCase();
    if (q) {
      // Free-text across everything an operator would grep for: ids, tenant,
      // worker, type, and the error text itself.
      all = all.filter((r) =>
        [
          r.task_id,
          r.run_id,
          r.task_type,
          r.worker_id,
          r.workspace_name,
          r.org_name,
          r.originating_user_email,
          r.run_error_message
        ]
          .filter(Boolean)
          .some((field) => (field as string).toLowerCase().includes(q))
      );
    }
    return all;
  }, [data, filter, query]);

  const deadIds = useMemo(
    () => rows.filter((r) => r.queue_status === "dead").map((r) => r.task_id),
    [rows]
  );

  // Drop selections that have left the visible/filtered set so the bulk bar
  // count stays honest.
  useEffect(() => {
    const live = new Set(deadIds);
    setSelected((prev) => {
      const next = new Set<string>();
      prev.forEach((id) => {
        if (live.has(id)) next.add(id);
      });
      return next.size === prev.size ? prev : next;
    });
  }, [deadIds]);

  const counts = useMemo(() => {
    const all = data ?? [];
    return {
      all: all.length,
      failed: all.filter((r) => r.queue_status === "failed").length,
      dead: all.filter((r) => r.queue_status === "dead").length
    };
  }, [data]);

  const toggleSelect = (taskId: string) =>
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(taskId)) next.delete(taskId);
      else next.add(taskId);
      return next;
    });

  const toggleExpand = (taskId: string) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(taskId)) next.delete(taskId);
      else next.add(taskId);
      return next;
    });

  const allDeadSelected = deadIds.length > 0 && deadIds.every((id) => selected.has(id));
  const toggleAllDead = () =>
    setSelected((prev) => {
      if (allDeadSelected) return new Set([...prev].filter((id) => !deadIds.includes(id)));
      const next = new Set(prev);
      for (const id of deadIds) next.add(id);
      return next;
    });

  const runBulk = async (kind: "reenqueue" | "delete") => {
    const ids = [...selected];
    if (ids.length === 0) return;
    if (kind === "delete" && !window.confirm(`Permanently delete ${ids.length} dead task(s)?`))
      return;
    setIsBulking(kind);
    try {
      const op =
        kind === "reenqueue" ? InternalJobsService.reenqueueDead : InternalJobsService.deleteDead;
      const results = await Promise.allSettled(ids.map((id) => op(id)));
      const ok = results.filter((r) => r.status === "fulfilled").length;
      const failed = results.length - ok;
      const verb = kind === "reenqueue" ? "Re-enqueued" : "Deleted";
      if (ok > 0 && failed === 0) toast.success(`${verb} ${ok} task${ok === 1 ? "" : "s"}`);
      else if (ok > 0) toast.success(`${verb} ${ok} / ${results.length} (${failed} failed)`);
      else toast.error(`Failed to ${kind === "reenqueue" ? "re-enqueue" : "delete"} ${failed}`);
      qc.invalidateQueries({ queryKey: queryKeys.internalJobs.all });
      setSelected(new Set());
    } finally {
      setIsBulking(null);
    }
  };

  const deleteOne = (taskId: string) => {
    if (window.confirm(`Permanently delete dead task ${taskId}?`)) remove.mutate(taskId);
  };

  return (
    <section className='space-y-3' data-testid='admin-internal-jobs-console'>
      <header className='flex flex-wrap items-center justify-between gap-3'>
        <div className='flex items-center gap-3'>
          <h3 className='font-medium text-[10px] text-muted-foreground uppercase tracking-[0.14em]'>
            Jobs
          </h3>
          <div className='flex items-center gap-1'>
            {FILTERS.map((f) => (
              <button
                key={f.id}
                type='button'
                onClick={() => setFilter(f.id)}
                className={cn(
                  "rounded-md px-2 py-1 font-medium text-[11px] uppercase tracking-wide transition-colors",
                  filter === f.id
                    ? "bg-foreground text-background"
                    : "text-muted-foreground hover:bg-muted hover:text-foreground"
                )}
              >
                {f.label}
                <span className='ml-1.5 tabular-nums opacity-60'>{counts[f.id]}</span>
              </button>
            ))}
          </div>
        </div>
        <div className='flex items-center gap-3'>
          <div className='relative'>
            <Search className='absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground' />
            <input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder='Filter by task, workspace, org, worker, error…'
              className='h-7 w-64 rounded-md border border-border/60 bg-card pr-2 pl-7 font-mono text-[11px] outline-none placeholder:text-muted-foreground/60 focus:border-border focus:ring-1 focus:ring-ring'
              aria-label='Filter jobs'
            />
          </div>
          {deadIds.length > 0 ? (
            <button
              type='button'
              onClick={toggleAllDead}
              className='flex shrink-0 items-center gap-2 text-[11px] text-muted-foreground hover:text-foreground'
            >
              <Checkbox checked={allDeadSelected} aria-label='Select all dead' />
              Select all dead
            </button>
          ) : null}
        </div>
      </header>

      {/* `isEmpty` reads `rows`, not the fetched array: the filter and the search
          box narrow client-side, so "nothing to show" is a property of the view,
          not of the response. */}
      <AdminAsync
        query={failures}
        noun='jobs'
        skeleton={<Skeleton className='h-48 w-full' />}
        isEmpty={() => rows.length === 0}
        empty={
          <AdminEmptyState
            icon={CheckCircle2}
            title={
              query.trim()
                ? `No jobs match "${query.trim()}".`
                : filter === "all"
                  ? "No failed or dead jobs."
                  : `No ${filter} jobs.`
            }
            description={query.trim() ? "Try a broader filter." : "The worker fleet is healthy."}
          />
        }
      >
        {() => (
          <div className='overflow-hidden rounded-lg border border-border/60 bg-card'>
            <div className='grid grid-cols-[auto_auto_minmax(0,1fr)_auto_auto_auto_auto] gap-3 border-border/60 border-b bg-muted/30 px-3 py-2 font-medium text-[10px] text-muted-foreground uppercase tracking-[0.14em]'>
              <span className='w-4' aria-hidden />
              <span>Type · Status</span>
              <span>Workspace · Org</span>
              <span className='max-w-40'>Worker</span>
              <span>Claims</span>
              <span className='w-14 text-right'>Age</span>
              <span className='w-12' aria-hidden />
            </div>
            <div className='divide-y divide-border/50'>
              {rows.map((row) => (
                <JobRow
                  key={row.task_id}
                  row={row}
                  selected={selected.has(row.task_id)}
                  onToggleSelect={() => toggleSelect(row.task_id)}
                  expanded={expanded.has(row.task_id)}
                  onToggleExpand={() => toggleExpand(row.task_id)}
                  onRetry={() => reenqueue.mutate(row.task_id)}
                  onDelete={() => deleteOne(row.task_id)}
                  retrying={reenqueue.isPending && reenqueue.variables === row.task_id}
                  deleting={remove.isPending && remove.variables === row.task_id}
                />
              ))}
            </div>
          </div>
        )}
      </AdminAsync>

      <BulkActionBar
        selectedCount={selected.size}
        onClear={() => setSelected(new Set())}
        onReenqueue={() => runBulk("reenqueue")}
        onDelete={() => runBulk("delete")}
        isReenqueueing={isBulking === "reenqueue"}
        isDeleting={isBulking === "delete"}
      />
    </section>
  );
};
