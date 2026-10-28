import { Handle, type NodeProps, Position } from "@xyflow/react";
import { ChevronRight } from "lucide-react";
import { cn } from "@/libs/shadcn/utils";
import type { WmComputedMeasure, WorldModelEntity } from "@/types/worldModel";
import { GraphNodeCard, GraphNodeHandles } from "../../components/semanticGraph";
import { contributorHandleId, measureSymbol, measureSymbolColor } from "../worldModelLayout";

interface WmEntityData {
  entity: WorldModelEntity;
  /** The customer's read-only page (see `WorldModelGraph`'s `plain`). */
  plain?: boolean;
  selected: boolean;
  dimmed: boolean;
  /** Softly blurred into the background while an unrelated breakdown is shown. */
  blurred?: boolean;
  filterCount?: {
    matched: number;
    total: number;
    sample?: string[];
    sample_keys?: string[];
  } | null;
  isCountLoading?: boolean;
  /** Provided only for the filter-seed entity when a filter is active. */
  seedComputedMeasures?: WmComputedMeasure[] | null;
  /** True when `seedComputedMeasures` are breakdown contributors (not the
   *  filter-seed's own chips), so their rows expose per-measure edge handles. */
  isContributorCard?: boolean;
  /** Expand this entity into the driver-tree breakdown for `measure`. */
  onExpandEntity?: (id: string | null, measure?: string | null) => void;
  /** Select one of this entity's sampled descendant rows as the new instance. */
  onSelectChildInstance?: (entityId: string, key: string, display: string) => void;
  /** Open the searchable browser for all of this entity's reachable rows. */
  onBrowseSamples?: (entityId: string, position: { x: number; y: number }) => void;
}

function formatMeasureValue(raw: string): string {
  const n = Number(raw);
  if (!Number.isFinite(n)) return raw;
  if (Number.isInteger(n)) return n.toLocaleString();
  const formatted = n.toPrecision(7).replace(/\.?0+$/, "");
  return Number(formatted).toLocaleString(undefined, { maximumFractionDigits: 4 });
}

function MetricChipsSection({
  measures,
  entity,
  onExpandEntity,
  withMeasureHandles = false
}: {
  measures: WmComputedMeasure[];
  entity: WorldModelEntity;
  onExpandEntity?: (id: string | null, measure?: string | null) => void;
  /** When these chips are breakdown contributors, expose a per-measure source
   *  handle on each row so the breakdown edge attaches to the contributor
   *  number itself rather than to the card as a whole. */
  withMeasureHandles?: boolean;
}) {
  if (measures.length === 0) {
    return (
      <div className='mt-0.5 border-border border-t pt-1 font-mono text-[9px] text-muted-foreground'>
        no measures
      </div>
    );
  }

  return (
    <div className='mt-0.5 flex flex-col gap-0.5 border-border border-t pt-1'>
      {measures.map((m) => {
        const def =
          entity.own_measures.find((d) => d.name === m.name) ??
          entity.induced_measures.find((d) => d.name === m.name);
        const isNonAdditive = def?.additivity === "non_additive";
        const hasBreakdown = def?.has_breakdown === true;

        return (
          <div
            key={m.name}
            className='group relative flex min-w-0 items-center gap-1 font-mono text-[9.5px]'
          >
            {withMeasureHandles && (
              <Handle
                type='source'
                id={contributorHandleId(m.name)}
                position={Position.Right}
                className='!h-1.5 !w-1.5 !min-w-0 !border-0 !bg-[color:var(--vis-purple)] opacity-0'
                style={{ right: -4 }}
              />
            )}
            <span
              className={cn(
                "shrink-0 text-[10px] leading-none",
                measureSymbolColor(m.measure_type)
              )}
            >
              {measureSymbol(m.measure_type)}
            </span>
            <span className='min-w-0 flex-1 truncate text-muted-foreground'>
              {m.label ?? m.name}
            </span>
            {m.value === null ? (
              <span className='h-2 w-10 animate-pulse rounded bg-muted' />
            ) : (
              <span className='shrink-0 text-foreground tabular-nums'>
                {formatMeasureValue(m.value)}
              </span>
            )}
            {isNonAdditive && (
              <span className='shrink-0 text-[8px] text-status-warning' title='non-additive'>
                ⚠
              </span>
            )}
            {onExpandEntity && hasBreakdown && (
              <button
                type='button'
                className='shrink-0 text-info transition-colors hover:text-foreground'
                title='Break down at this instance'
                data-testid={`wm-measure-zoom-${entity.id}-${m.name}`}
                onClick={(e) => {
                  e.stopPropagation();
                  onExpandEntity(entity.id, m.name);
                }}
              >
                <ChevronRight className='size-3' />
              </button>
            )}
          </div>
        );
      })}
    </div>
  );
}

export function WorldModelEntityNode({ data }: NodeProps) {
  const {
    entity,
    selected,
    dimmed,
    blurred,
    filterCount,
    isCountLoading,
    seedComputedMeasures,
    isContributorCard,
    onExpandEntity,
    onSelectChildInstance,
    onBrowseSamples,
    plain
  } = data as unknown as WmEntityData;

  const obsCount = entity.dimensions.length;
  const calcCount = entity.own_measures.length + entity.induced_measures.length;
  const isUnreachable = !!filterCount && filterCount.matched === 0;

  return (
    <>
      <GraphNodeHandles />
      <GraphNodeCard
        selected={selected}
        dimmed={dimmed}
        blurred={blurred}
        data-testid={`wm-entity-${entity.id}`}
      >
        {/* Row 1: entity name */}
        <div className='flex items-baseline justify-between gap-1.5'>
          <span
            className={cn(
              "truncate font-medium text-[12px] leading-tight",
              dimmed ? "text-muted-foreground" : "text-foreground"
            )}
          >
            {entity.label}
          </span>
        </div>

        {/* Row 2: grain id · depth (not on the customer's page) */}
        {!plain && (
          <div className='flex min-w-0 items-center gap-1.5 font-mono text-[9px] text-muted-foreground'>
            <span
              className={cn(
                "size-1.5 shrink-0 rounded-full",
                dimmed ? "bg-muted-foreground" : "bg-info"
              )}
            />
            <span className='truncate'>{entity.id}</span>
            <span className='shrink-0 opacity-50'>·</span>
            <span className='shrink-0'>depth {entity.depth}</span>
          </div>
        )}

        {/* Row 3: metric chips, filter badge, loading skeleton, or obs/calc counts */}
        <div className='mt-0.5 text-[10px]'>
          {seedComputedMeasures ? (
            <MetricChipsSection
              measures={seedComputedMeasures}
              entity={entity}
              onExpandEntity={onExpandEntity}
              withMeasureHandles={isContributorCard}
            />
          ) : filterCount ? (
            <div className='flex flex-col gap-1'>
              <div
                className={cn(
                  "flex w-full items-baseline justify-between gap-1 border px-1.5 py-0.5 font-mono text-[10px]",
                  isUnreachable
                    ? "border-destructive/40 text-muted-foreground"
                    : "border-info/30 text-info"
                )}
              >
                <span className='uppercase tracking-wider'>filter</span>
                <span className='tabular-nums'>
                  {filterCount.matched}
                  <span className='text-muted-foreground'> / {filterCount.total}</span>
                </span>
              </div>
              {filterCount.sample && filterCount.sample.length > 0 && (
                <div className='flex flex-col gap-1'>
                  {filterCount.sample.map((display, i) => {
                    const key = filterCount.sample_keys?.[i] ?? display;
                    return (
                      <button
                        key={key}
                        type='button'
                        className='flex w-full min-w-0 items-center gap-1 border border-border bg-background/60 px-1.5 py-0.5 font-mono text-[9.5px] transition-colors hover:border-info/60'
                        data-testid={`wm-entity-child-${entity.id}-${key}`}
                        onClick={(e) => {
                          e.stopPropagation();
                          onSelectChildInstance?.(entity.id, key, display);
                        }}
                        title={display}
                      >
                        <span className='shrink-0 text-info'>↓</span>
                        <span className='min-w-0 flex-1 truncate text-left text-foreground'>
                          {display}
                        </span>
                      </button>
                    );
                  })}
                  {filterCount.matched > filterCount.sample.length && (
                    <button
                      type='button'
                      className='flex w-full items-center justify-center gap-1 border border-info/30 border-dashed bg-background/40 px-1.5 py-0.5 font-mono text-[9px] text-info uppercase tracking-wider transition-colors hover:border-info/70 hover:bg-info/5'
                      data-testid={`wm-entity-browse-${entity.id}`}
                      onClick={(e) => {
                        e.stopPropagation();
                        onBrowseSamples?.(entity.id, { x: e.clientX, y: e.clientY });
                      }}
                    >
                      + {(filterCount.matched - filterCount.sample.length).toLocaleString()} more
                    </button>
                  )}
                </div>
              )}
            </div>
          ) : isCountLoading ? (
            <div className='flex w-full items-baseline justify-between border border-info/20 px-1.5 py-0.5 font-mono text-[10px] text-info/50'>
              <span className='uppercase tracking-wider'>filtering</span>
              <span className='animate-pulse'>···</span>
            </div>
          ) : plain ? (
            <span className='text-muted-foreground'>
              {calcCount} {calcCount === 1 ? "measure" : "measures"}, {obsCount}{" "}
              {obsCount === 1 ? "detail" : "details"}
            </span>
          ) : (
            <div className='flex items-center gap-3'>
              <span className='flex items-center gap-1'>
                <span className='text-success'>●</span>
                <span className='text-foreground'>{obsCount}</span>
                <span className='text-muted-foreground'>obs</span>
              </span>
              <span className='flex items-center gap-1'>
                <span className='font-mono text-[11px] text-[color:var(--vis-purple)]'>Σ</span>
                <span className='text-foreground'>{calcCount}</span>
                <span className='text-muted-foreground'>calc</span>
              </span>
            </div>
          )}
        </div>
      </GraphNodeCard>
    </>
  );
}
