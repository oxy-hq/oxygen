import { useCallback, useState } from "react";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { useWmFilterCounts, useWmInstanceDetail, useWorldModel } from "@/hooks/api/useWorldModel";
import { cn } from "@/libs/shadcn/utils";
import type { WmFilterSeed, WmInstance, WmSelection } from "@/types/worldModel";
import { PANEL_WIDTH } from "../components/semanticGraph";
import { FilterPill } from "./components/FilterPill";
import { InstancePickerPopover } from "./components/InstancePickerPopover";
import { SampleBrowserPopover } from "./components/SampleBrowserPopover";
import { WorldModelDetailPanel } from "./components/WorldModelDetailPanel";
import { WorldModelGraph } from "./components/WorldModelGraph";

export default function WorldModelView() {
  const { data: model, isLoading, error } = useWorldModel();

  // Selection + back-navigation history stack.
  const [selection, setSelectionRaw] = useState<WmSelection>(null);
  const [history, setHistory] = useState<WmSelection[]>([]);

  const [filterSeed, setFilterSeed] = useState<WmFilterSeed | null>(null);

  // Picker state: which entity + screen position.
  const [pickerState, setPickerState] = useState<{
    entityId: string;
    position: { x: number; y: number };
  } | null>(null);

  // Sample-browser state: which descendant entity's reachable rows to browse.
  const [browserState, setBrowserState] = useState<{
    entityId: string;
    position: { x: number; y: number };
  } | null>(null);

  // Driver-tree expansion: which entity node is expanded + which measure it breaks down.
  const [expandedEntityId, setExpandedEntityId] = useState<string | null>(null);
  const [breakdownMeasure, setBreakdownMeasure] = useState<string | null>(null);

  const onExpandEntity = useCallback((entityId: string | null, measure?: string | null) => {
    setExpandedEntityId(entityId);
    setBreakdownMeasure(entityId ? (measure ?? null) : null);
  }, []);

  // Expanding a measure from the sidebar drives the same state as the graph
  // node's own expand button — and since the sidebar can be showing an
  // instance that isn't the active filter seed, promote it to the seed too
  // so the node's breakdown is valued at the instance actually being viewed.
  const onExpandMeasureFromDetail = useCallback(
    (entityId: string, keyValue: string, label: string, measure: string | null) => {
      if (measure) {
        setFilterSeed({ entityId, keyValue, label });
        setExpandedEntityId(entityId);
        setBreakdownMeasure(measure);
      } else {
        setExpandedEntityId(null);
        setBreakdownMeasure(null);
      }
    },
    []
  );

  // Stream filter counts lazily when a seed is set — counts appear progressively.
  const {
    counts: filterCounts,
    isLoading: isCountLoading,
    error: countError
  } = useWmFilterCounts(filterSeed?.entityId ?? null, filterSeed?.keyValue ?? null);

  // Load computed measures for the filter seed so entity card chips can show values.
  // The detail panel reads the same instance through the same hook, which opens
  // one stream for both; a failure is this page's to report too, because the
  // panel may be showing something other than the seed.
  const { data: seedDetail, error: seedDetailError } = useWmInstanceDetail(
    filterSeed?.entityId ?? null,
    filterSeed?.keyValue ?? null
  );

  const onSelect = useCallback((next: WmSelection) => {
    setSelectionRaw((prev) => {
      if (next === null) {
        setHistory([]);
        return null;
      }
      if (prev !== null) {
        setHistory((h) => [...h, prev]);
      }
      return next;
    });
  }, []);

  const onBack = useCallback(() => {
    setHistory((h) => {
      const prev = h[h.length - 1] ?? null;
      setSelectionRaw(prev);
      return h.slice(0, -1);
    });
  }, []);

  const handleOpenPicker = useCallback((entityId: string, position: { x: number; y: number }) => {
    setPickerState({ entityId, position });
  }, []);

  const handlePickInstance = useCallback(
    (inst: WmInstance) => {
      if (!pickerState) return;
      const seed: WmFilterSeed = {
        entityId: pickerState.entityId,
        keyValue: inst.key,
        label: inst.display
      };
      setFilterSeed(seed);
      onSelect({
        kind: "instance",
        entityId: pickerState.entityId,
        keyValue: inst.key,
        label: inst.display
      });
    },
    [pickerState, onSelect]
  );

  const handleSelectChildInstance = useCallback(
    (entityId: string, key: string, display: string) => {
      setFilterSeed({ entityId, keyValue: key, label: display });
      onSelect({ kind: "instance", entityId, keyValue: key, label: display });
    },
    [onSelect]
  );

  const handleBrowseSamples = useCallback(
    (entityId: string, position: { x: number; y: number }) => {
      setBrowserState({ entityId, position });
    },
    []
  );

  const handlePickBrowsedInstance = useCallback(
    (inst: WmInstance) => {
      if (!browserState) return;
      handleSelectChildInstance(browserState.entityId, inst.key, inst.display);
      setBrowserState(null);
    },
    [browserState, handleSelectChildInstance]
  );

  const handleClearFilter = useCallback(() => {
    setFilterSeed(null);
    setExpandedEntityId(null);
    setBreakdownMeasure(null);
    setBrowserState(null);
    setSelectionRaw((prev) => (prev?.kind === "instance" ? null : prev));
    setHistory((h) => h.filter((s) => s?.kind !== "instance"));
  }, []);

  if (isLoading) {
    return (
      <div className='flex h-full w-full flex-col items-center justify-center gap-3 text-muted-foreground'>
        <Spinner className='size-6' />
        <p className='text-sm'>Loading world model…</p>
      </div>
    );
  }

  if (error || !model) {
    return (
      <div className='flex h-full w-full items-center justify-center text-destructive text-sm'>
        {error instanceof Error ? error.message : "Failed to load world model"}
      </div>
    );
  }

  const pickerEntity = pickerState
    ? model.entities.find((e) => e.id === pickerState.entityId)
    : null;
  const browserEntity = browserState
    ? model.entities.find((e) => e.id === browserState.entityId)
    : null;

  return (
    <div className='flex h-full min-h-0 w-full overflow-hidden'>
      <div className='relative min-h-0 flex-1 overflow-hidden'>
        {filterSeed && (
          <FilterPill
            seed={filterSeed}
            isCountLoading={isCountLoading}
            countFailed={!!countError}
            measuresFailed={!!seedDetailError}
            onClear={handleClearFilter}
          />
        )}
        <WorldModelGraph
          model={model}
          selection={selection}
          filterCounts={filterCounts}
          isCountLoading={isCountLoading}
          filterSeedEntityId={filterSeed?.entityId ?? null}
          seedComputedMeasures={seedDetail?.computed_measures ?? null}
          expandedEntityId={expandedEntityId}
          breakdownMeasure={breakdownMeasure}
          instanceKey={filterSeed?.keyValue ?? null}
          onExpandEntity={onExpandEntity}
          onSelectEntity={(id) => onSelect({ kind: "entity", entityId: id })}
          onSelectPromotion={(from, to) => onSelect({ kind: "promotion", from, to })}
          onOpenPicker={handleOpenPicker}
          onClearSelection={() => onSelect(null)}
          onSelectChildInstance={handleSelectChildInstance}
          onBrowseSamples={handleBrowseSamples}
        />
        {pickerState && pickerEntity && (
          <InstancePickerPopover
            entityId={pickerState.entityId}
            entityLabel={pickerEntity.label}
            position={pickerState.position}
            onPick={handlePickInstance}
            onClose={() => setPickerState(null)}
          />
        )}
        {browserState && browserEntity && filterSeed && (
          <SampleBrowserPopover
            seedEntityId={filterSeed.entityId}
            seedKey={filterSeed.keyValue}
            entityId={browserState.entityId}
            entityLabel={browserEntity.label}
            matched={filterCounts?.[browserState.entityId]?.matched ?? 0}
            position={browserState.position}
            onPick={handlePickBrowsedInstance}
            onClose={() => setBrowserState(null)}
          />
        )}
      </div>
      <div className={cn(PANEL_WIDTH, "shrink-0 overflow-hidden border-border border-l")}>
        <WorldModelDetailPanel
          model={model}
          selection={selection}
          onSelect={onSelect}
          history={history}
          onBack={onBack}
          seedInstanceDetail={seedDetail ?? null}
          expandedEntityId={expandedEntityId}
          breakdownMeasure={breakdownMeasure}
          onExpandMeasure={onExpandMeasureFromDetail}
        />
      </div>
    </div>
  );
}
