import { Activity, TriangleAlert } from "lucide-react";
import { useState } from "react";
import { useNavigate } from "react-router-dom";
import TablePagination from "@/components/ui/TablePagination";
import { useAuth } from "@/contexts/AuthContext";
import ROUTES from "@/libs/utils/routes";
import PageHeader from "@/pages/ide/components/PageHeader";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import TraceCharts from "./components/Charts";
import { CompareSelectionBar } from "./components/CompareSelectionBar";
import { CompareTracesDialog } from "./components/CompareTracesDialog";
import { TimeRangeControl } from "./components/TimeRangeControl";
import TracesList from "./components/TracesList";
import { TracesToolbar } from "./components/TracesToolbar";
import { useTracesController } from "./useTracesController";

function ObservabilityNotConfiguredBanner() {
  return (
    <div className='mb-4 flex gap-3 rounded-md border border-warning/30 bg-warning/5 p-4'>
      <div className='flex size-8 shrink-0 items-center justify-center rounded-full bg-warning/15 text-warning'>
        <TriangleAlert className='size-4' />
      </div>
      <div className='flex min-w-0 flex-1 flex-col gap-2'>
        <div>
          <div className='font-medium text-sm'>Observability is not configured</div>
          <p className='mt-0.5 text-muted-foreground text-sm'>
            No traces will be recorded. Enable the ClickHouse backend and restart the server.
          </p>
        </div>
        <pre className='overflow-x-auto rounded-sm bg-muted px-3 py-2 font-mono text-xs leading-relaxed'>
          <span className='text-muted-foreground'>
            # ClickHouse is the sole backend (oxy start boots the container)
          </span>
          {"\n"}
          <span>export </span>
          <span className='text-warning'>OXY_OBSERVABILITY_BACKEND</span>
          <span>=clickhouse</span>
        </pre>
      </div>
    </div>
  );
}

export default function TracesPage() {
  const navigate = useNavigate();
  const { workspace: project } = useCurrentWorkspace();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const { authConfig } = useAuth();
  const observabilityConfigured = authConfig.observability_enabled;
  const [compareOpen, setCompareOpen] = useState(false);

  const ctl = useTracesController({ enabled: observabilityConfigured });
  const totalPages = Math.ceil(ctl.total / ctl.pageSize);

  const goToTrace = (traceId: string) =>
    navigate(
      ROUTES.ORG(orgSlug)
        .WORKSPACE(project?.id || "")
        .IDE.OBSERVABILITY.TRACE(traceId)
    );

  const timeRangeAction = observabilityConfigured ? (
    <TimeRangeControl value={ctl.timeRange} onChange={ctl.setTimeRange} />
  ) : null;

  return (
    <div className='flex h-full flex-col'>
      <div className='flex min-h-0 flex-1 flex-col'>
        <PageHeader icon={Activity} title='Traces' actions={timeRangeAction} />

        <div className='scrollbar-gutter-auto min-h-0 flex-1 overflow-auto p-4'>
          {observabilityConfigured ? (
            <>
              <TraceCharts
                traces={ctl.chartTraces}
                total={ctl.chartTotal}
                isLoading={ctl.isChartLoading}
              />
              <TracesToolbar
                search={ctl.searchInput}
                onSearchChange={ctl.setSearchInput}
                status={ctl.status}
                onStatusChange={ctl.setStatus}
                live={ctl.live}
                onLiveChange={ctl.setLive}
                view={ctl.view}
                onViewChange={ctl.setView}
              />
              <CompareSelectionBar
                count={ctl.compareTraces.length}
                onCompare={() => ctl.compareTraces.length === 2 && setCompareOpen(true)}
                onClear={ctl.clearSelection}
              />
              <TracesList
                isLoading={ctl.isLoading}
                traces={ctl.traces}
                searchQuery={ctl.filtersActive ? "filtered" : ""}
                onTraceClick={goToTrace}
                view={ctl.view}
                selectedIds={ctl.selectedIds}
                onToggleSelect={ctl.toggleSelect}
              />
            </>
          ) : (
            <ObservabilityNotConfiguredBanner />
          )}
        </div>

        {observabilityConfigured && !ctl.isLoading && (
          <div className='p-5'>
            <TablePagination
              currentPage={ctl.currentPage}
              totalPages={totalPages}
              totalItems={ctl.total}
              pageSize={ctl.pageSize}
              onPageChange={ctl.handlePageChange}
              itemLabel='traces'
            />
          </div>
        )}
      </div>

      <CompareTracesDialog
        traces={ctl.compareTraces}
        open={compareOpen}
        onOpenChange={setCompareOpen}
        onOpenTrace={(id) => {
          setCompareOpen(false);
          // `goToTrace` is react-router's `navigate`, whose promise only marks the
          // navigation settling.
          void goToTrace(id);
        }}
      />
    </div>
  );
}
