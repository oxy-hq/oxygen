import { useMemo, useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { usePublishApp, useUnpublishApp } from "@/hooks/api/customApps/useCustomApps";
import { useAppHealthIndex } from "@/hooks/api/customApps/useFleetHealth";
import { useRowSelection } from "@/hooks/useRowSelection";
import type { CustomApp } from "@/types/apps";
import { AppsGallery, type AppsViewProps } from "./components/AppsGallery";
import { AppsListView } from "./components/AppsListView";
import { AppsToolbar } from "./components/AppsToolbar";
import { BulkActionBar } from "./components/BulkActionBar";
import {
  buildAppsTableModel,
  defaultDirFor,
  type SortKey,
  statusOf,
  useAppsTableState
} from "./useAppsTable";

interface AppsTableProps {
  apps: CustomApp[];
  isLoading: boolean;
  /** True while background pages are still streaming in (auto-load-all). */
  isLoadingMore: boolean;
  onSelect: (app: CustomApp) => void;
  onCreate: () => void;
}

/**
 * The custom-app registry, with each app's health joined on: a toolbar, a list
 * **or** cards over the same filtered model, row selection, and a sticky bulk-
 * action bar. Rich per-app detail opens as a full page via `onSelect`.
 *
 * Health used to be its own tab listing the same apps with different facts. It
 * is a column here now, because "is this app OK?" is one question and an
 * operator should not have to cross-reference two tables to answer it.
 */
export const AppsTable = ({
  apps,
  isLoading,
  isLoadingMore,
  onSelect,
  onCreate
}: AppsTableProps) => {
  const [state, setState] = useAppsTableState();
  const health = useAppHealthIndex();
  const model = useMemo(
    () => buildAppsTableModel(apps, state, health.rows),
    [apps, state, health.rows]
  );
  const selection = useRowSelection(model.flatIds);
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());

  const publishApp = usePublishApp();
  const unpublishApp = useUnpublishApp();

  const showOrg = state.group !== "org";
  const showGroupHeaders = state.group !== "none";

  const onSort = (key: SortKey) => {
    if (state.sortKey === key) {
      setState({ sortDir: state.sortDir === "asc" ? "desc" : "asc" });
    } else {
      setState({ sortKey: key, sortDir: defaultDirFor(key) });
    }
  };

  const toggleCollapse = (key: string) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });

  const viewProps: AppsViewProps = {
    groups: model.groups,
    statusOf: (a) => statusOf(a, health.rows),
    health: health.rows,
    selecting: selection.someSelected || selection.allSelected,
    showOrg,
    showGroupHeaders,
    collapsed,
    onToggleCollapse: toggleCollapse,
    isSelected: selection.isSelected,
    onToggleRow: selection.toggle,
    onToggleGroup: selection.setMany,
    onOpen: onSelect,
    onPublish: (a) => publishApp.mutate(a.id),
    onUnpublish: (a) => unpublishApp.mutate(a.id)
  };

  return (
    <div className='flex h-full min-h-0 flex-col'>
      <AppsToolbar
        state={state}
        setState={setState}
        onCreate={onCreate}
        counts={model.statusCounts}
        scopedTotal={countScoped(model.statusCounts)}
        orgs={model.orgs}
        healthNote={healthNote(health)}
      />

      {isLoading ? (
        <CenteredState>
          <span className='flex items-center gap-2'>
            <Spinner className='size-4' /> Loading apps…
          </span>
        </CenteredState>
      ) : model.totalCount === 0 ? (
        <CenteredState>
          <EmptyState onCreate={onCreate} />
        </CenteredState>
      ) : model.filteredCount === 0 ? (
        <CenteredState>
          <div className='flex flex-col items-center gap-2' data-testid='admin-apps-no-match'>
            <p>No apps match these filters.</p>
            <Button
              size='sm'
              variant='outline'
              onClick={() => setState({ q: "", status: "all", org: "all" })}
            >
              Clear filters
            </Button>
          </div>
        </CenteredState>
      ) : state.view === "gallery" ? (
        <AppsGallery {...viewProps} />
      ) : (
        <AppsListView
          {...viewProps}
          sortKey={state.sortKey}
          sortDir={state.sortDir}
          onSort={onSort}
          allSelected={selection.allSelected}
          someSelected={selection.someSelected}
          onToggleAll={selection.toggleAll}
        />
      )}

      {isLoadingMore && (
        <div className='flex shrink-0 items-center justify-center gap-2 border-t py-1.5 text-muted-foreground text-xs'>
          <Spinner className='size-3' /> Loading all apps…
        </div>
      )}

      <BulkActionBar selectedIds={selection.selectedIds} onClear={selection.clear} />
    </div>
  );
};

const CenteredState = ({ children }: { children: React.ReactNode }) => (
  <div className='flex min-h-0 flex-1 items-center justify-center p-12 text-center text-muted-foreground text-xs'>
    {children}
  </div>
);

const EmptyState = ({ onCreate }: { onCreate: () => void }) => (
  <div className='flex flex-col items-center gap-2' data-testid='admin-apps-empty'>
    <p>No custom apps yet.</p>
    <Button size='sm' variant='outline' onClick={onCreate}>
      Create the first app
    </Button>
  </div>
);

/** Every app the non-status filters let through: the `All` chip's number. */
const countScoped = (c: ReturnType<typeof buildAppsTableModel>["statusCounts"]) =>
  c.down + c.degraded + c.not_measured + c.quiet + c.operational + c.draft + c.unknown;

/**
 * Why published apps might show no status, said once for the whole list rather
 * than as a blank down a column. Each case has a different fix, so each names
 * its own. `null` when nothing needs saying.
 */
function healthNote(h: ReturnType<typeof useAppHealthIndex>): string | null {
  if (h.isError) return "Health could not be loaded — statuses are unknown, not healthy.";
  if (h.isLoading) return "Checking health…";
  if (!h.captureConfigured) return "Observability capture is off, so no app is being measured.";
  if (h.hasMore)
    return `Health covers the first ${h.rows?.size ?? 0} of ${h.total} published apps.`;
  return null;
}
