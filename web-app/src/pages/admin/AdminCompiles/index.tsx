import { useQueryClient } from "@tanstack/react-query";
import { FileCheck, Hammer, Loader2 } from "lucide-react";
import { useMemo } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/shadcn/button";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { useBackfillUncompiled } from "@/hooks/api/compiles";
import queryKeys from "@/hooks/api/queryKey";
import { useRowSelection } from "@/hooks/useRowSelection";
import { LiveIndicator } from "../AdminInternalJobs/components/LiveIndicator";
import { AdminAsync } from "../components/AdminAsync";
import { AdminEmptyState } from "../components/AdminEmptyState";
import { AdminPage } from "../components/AdminPage";
import { BulkActionBar } from "./components/BulkActionBar";
import { CompileFilters, type CompileView } from "./components/CompileFilters";
import { RevisionTable } from "./components/RevisionTable";
import { RunCompileSheet } from "./components/RunCompileSheet";
import { WorkspaceTable } from "./components/WorkspaceTable";
import { useAdminCompiles } from "./useAdminCompiles";

// Stable empty list for the rollup-view revision selection (ids load lazily in
// child rows). A fresh `[]` each render would needlessly churn the hook's memos.
const NO_IDS: string[] = [];

/**
 * `/admin/compiles` — operator console for the compile boundary.
 *
 *   - Default "By workspace" rollup (one dense row per tenant) that
 *     expands into per-workspace revision history; "All revisions" keeps
 *     the original flat list.
 *   - 5s polling with a LiveIndicator pause/refresh.
 *   - Batch ops: recompile selected workspaces / promote selected
 *     revisions, both behind a confirm with a summarised toast.
 *   - "Run compile now" lives in a header Sheet to keep the grid dense.
 */
export default function AdminCompilesPage() {
  const qc = useQueryClient();
  const c = useAdminCompiles();

  // Three selection sets. Workspace selection drives "Recompile"; both
  // revision sets drive "Promote". They are mutually exclusive at the
  // bulk-bar level (see `bulk` below), so the operator never recompiles
  // and promotes in the same gesture.
  const wsSelection = useRowSelection(
    useMemo(() => c.workspaceRows.map((r) => r.workspace_id), [c.workspaceRows])
  );
  // Expanded-revision selection (rollup view). filterStale:false because these
  // ids are loaded lazily inside WorkspaceRow and never reach NO_IDS — without
  // it, every selection is filtered out and batch promote can't be triggered.
  const wsRevSelection = useRowSelection(NO_IDS, { filterStale: false });
  const flatSelection = useRowSelection(
    useMemo(() => c.revisionRows.map((r) => r.revision_id), [c.revisionRows])
  );

  // Flip the view and drop every selection in the same gesture so the
  // bulk bar can't carry stale ids across modes. Also clear the search box:
  // the rollup view free-text searches (q), but the flat view's only filter is
  // an exact workspace UUID — carrying rollup text over would filter to nothing.
  const onViewChange = (next: CompileView) => {
    wsSelection.clear();
    wsRevSelection.clear();
    flatSelection.clear();
    c.setQuery("");
    c.setView(next);
  };

  const onRefresh = () => qc.invalidateQueries({ queryKey: queryKeys.compiles.all });

  const backfill = useBackfillUncompiled();
  const onBackfill = () => {
    if (
      !window.confirm(
        "Enqueue a compile for every workspace that has never been compiled? This is the one-time backfill for projects that predate the compile boundary."
      )
    )
      return;
    backfill.mutate(undefined, {
      onSuccess: (res) =>
        toast.success(
          res.enqueued === 0
            ? "All workspaces are already compiled."
            : `Enqueued ${res.enqueued} compile${res.enqueued === 1 ? "" : "s"}.`
        ),
      onError: (e: unknown) => toast.error(e instanceof Error ? e.message : "Backfill failed")
    });
  };

  // Pick the active bulk action. Flat view → flat revisions. Rollup view →
  // workspaces, unless the operator instead selected expanded revisions.
  const bulk =
    c.view === "revisions"
      ? ({ mode: "revisions", sel: flatSelection } as const)
      : wsRevSelection.selectedIds.length > 0
        ? ({ mode: "revisions", sel: wsRevSelection } as const)
        : ({ mode: "workspace", sel: wsSelection } as const);

  // The table is tall, so bars would misrepresent it — keep the single block the
  // page already showed while loading.
  const tableSkeleton = <Skeleton className='h-40 w-full' />;

  return (
    // `pb-20` survives the frame swap: the BulkActionBar is `fixed bottom-4`, and
    // without the clearance it covers the last row of a full table.
    <AdminPage
      width='wide'
      className='pb-20'
      description={
        <>
          One row per workspace, expandable to its full revision history. GitHub pushes auto-enqueue
          compiles with <code className='font-mono'>promote=true</code>; use the tools here for
          ad-hoc, batch, and rollback operations.
        </>
      }
      actions={
        <>
          <RunCompileSheet />
          <Button
            size='sm'
            variant='outline'
            onClick={onBackfill}
            disabled={backfill.isPending}
            className='h-8 gap-1.5'
          >
            {backfill.isPending ? (
              <Loader2 className='size-3.5 animate-spin' />
            ) : (
              <Hammer className='size-3.5' />
            )}
            Compile all uncompiled
          </Button>
          <LiveIndicator
            updatedAt={c.active.dataUpdatedAt || undefined}
            paused={c.paused}
            onTogglePaused={() => c.setPaused((p) => !p)}
            onRefresh={onRefresh}
            isFetching={c.active.isFetching}
          />
        </>
      }
      data-testid='admin-compiles'
    >
      <section className='space-y-3' data-testid='admin-compiles-results'>
        <CompileFilters
          view={c.view}
          onViewChange={onViewChange}
          query={c.query}
          onQueryChange={c.setQuery}
          status={c.status}
          onStatusChange={c.setStatus}
          totalLabel={c.totalLabel}
        />

        {/* One AdminAsync per view rather than one over `c.active`: the two queries
            return different row types, and gating them separately keeps each
            branch's rows narrowed to its own table. */}
        {c.view === "workspace" ? (
          <AdminAsync
            query={c.workspaces}
            noun='compiles'
            skeleton={tableSkeleton}
            isEmpty={(d) => d.rows.length === 0}
            empty={
              <AdminEmptyState
                icon={FileCheck}
                title='No workspaces have compiled yet.'
                description='Run a compile or backfill to populate this view.'
              />
            }
          >
            {(d) => (
              <WorkspaceTable
                rows={d.rows}
                paused={c.paused}
                selection={wsSelection}
                revisionSelection={wsRevSelection}
              />
            )}
          </AdminAsync>
        ) : (
          <AdminAsync
            query={c.revisions}
            noun='compiles'
            skeleton={tableSkeleton}
            isEmpty={(d) => d.rows.length === 0}
            empty={
              <AdminEmptyState
                icon={FileCheck}
                title='No revisions yet.'
                description='The first compile will appear here once it runs.'
              />
            }
          >
            {(d) => <RevisionTable rows={d.rows} selection={flatSelection} />}
          </AdminAsync>
        )}
      </section>

      <BulkActionBar mode={bulk.mode} selectedIds={bulk.sel.selectedIds} onClear={bulk.sel.clear} />
    </AdminPage>
  );
}
