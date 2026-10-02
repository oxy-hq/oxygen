import { ArrowLeft, ChevronDown, ChevronRight, X } from "lucide-react";
import { useWmInstanceDetail } from "@/hooks/api/useWorldModel";
import { cn } from "@/libs/shadcn/utils";
import type {
  WmComputedMeasure,
  WmInstanceDetail,
  WmSelection,
  WorldModel,
  WorldModelDimension,
  WorldModelEdge,
  WorldModelEntity,
  WorldModelInducedMeasure,
  WorldModelMeasure
} from "@/types/worldModel";
import {
  formatMeasureValue,
  Row,
  SectionHeader,
  SectionSpinner
} from "../../components/semanticGraph";
import { measureSymbol, measureSymbolColor } from "../worldModelLayout";
import { WorldModelDriverTreeLive } from "./WorldModelDriverTree";
import { WorldModelInstanceOpportunity } from "./WorldModelInstanceOpportunity";
import { WorldModelMeasureAnalysis } from "./WorldModelMeasureAnalysis";

function EntityLink({
  entity,
  onSelect
}: {
  entity: WorldModelEntity;
  onSelect: (s: WmSelection) => void;
}) {
  return (
    <button
      type='button'
      className='flex w-full min-w-0 items-center gap-1.5 border border-border bg-background/40 px-2 py-1.5 text-left font-mono text-xs transition-colors hover:border-info/60'
      onClick={() => onSelect({ kind: "entity", entityId: entity.id })}
    >
      <span className='size-1.5 shrink-0 rounded-full bg-info' />
      <span className='truncate text-foreground'>{entity.label}</span>
      <span className='shrink-0 text-muted-foreground'>·</span>
      <span className='shrink-0 text-muted-foreground'>{entity.view}</span>
    </button>
  );
}

// ── Panel header ──────────────────────────────────────────────────────────────

function PanelHeader({
  breadcrumb,
  onBack,
  onClose
}: {
  breadcrumb: string;
  onBack?: () => void;
  onClose: () => void;
}) {
  return (
    <div className='flex shrink-0 items-center gap-2 border-border border-b bg-background px-3 py-2'>
      {onBack && (
        <button
          type='button'
          onClick={onBack}
          className='shrink-0 p-0.5 text-muted-foreground transition-colors hover:text-foreground'
          aria-label='Back'
        >
          <ArrowLeft size={14} />
        </button>
      )}
      <span className='min-w-0 flex-1 truncate font-mono text-[10px] text-foreground uppercase tracking-wider'>
        {breadcrumb}
      </span>
      <button
        type='button'
        onClick={onClose}
        className='flex h-5 w-5 shrink-0 items-center justify-center text-muted-foreground transition-colors hover:text-foreground'
        aria-label='Close'
      >
        <X size={14} />
      </button>
    </div>
  );
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/** Returns true if any edge along the induced measure's promotion path is non-functional. */
function isInducedMeasureBroken(
  measure: WorldModelInducedMeasure,
  edges: WorldModelEdge[]
): boolean {
  const path = measure.path;
  for (let i = 0; i < path.length - 1; i++) {
    const from = path[i];
    const to = path[i + 1];
    const edge = edges.find((e) => e.from === from && e.to === to);
    if (edge && !edge.functional) return true;
  }
  return false;
}

// ── Entity body ───────────────────────────────────────────────────────────────

function EntityBody({
  entity,
  model,
  onSelect
}: {
  entity: WorldModelEntity;
  model: WorldModel;
  onSelect: (s: WmSelection) => void;
}) {
  const outgoing = model.edges.filter((e) => e.from === entity.id);
  const incoming = model.edges.filter((e) => e.to === entity.id);

  return (
    <div className='flex flex-col gap-3 p-3'>
      {/* Title */}
      <div className='min-w-0'>
        <h2 className='truncate font-medium text-[16px] text-foreground'>{entity.label}</h2>
        <div className='mt-0.5 flex min-w-0 items-center gap-1.5 font-mono text-[10px] text-info uppercase tracking-wider'>
          <span className='shrink-0'>depth {entity.depth}</span>
          <span className='shrink-0 opacity-50'>·</span>
          <span className='truncate'>{entity.view}</span>
        </div>
        {entity.description && (
          <p className='mt-1.5 break-words text-[11px] text-muted-foreground leading-snug'>
            {entity.description}
          </p>
        )}
      </div>

      {/* Promotions in */}
      {incoming.length > 0 && (
        <section className='flex flex-col gap-1.5 border-border border-t pt-3'>
          <SectionHeader title='Promotions In' subtitle='Σ_p arrives from these entities' />
          <div className='flex flex-col gap-1'>
            {incoming.map((e) => (
              <PromotionRow
                key={e.from}
                edge={e}
                label={`${e.from} → ${entity.id}`}
                direction='in'
                onSelect={onSelect}
              />
            ))}
          </div>
        </section>
      )}

      {/* Promotions out */}
      {outgoing.length > 0 && (
        <section className='flex flex-col gap-1.5 border-border border-t pt-3'>
          <SectionHeader title='Promotions Out' subtitle='this entity promotes up to' />
          <div className='flex flex-col gap-1'>
            {outgoing.map((e) => (
              <PromotionRow
                key={e.to}
                edge={e}
                label={`${entity.id} → ${e.to}`}
                direction='out'
                onSelect={onSelect}
              />
            ))}
          </div>
        </section>
      )}

      {/* Observed attributes */}
      <section className='flex flex-col gap-1.5 border-border border-t pt-3'>
        <SectionHeader
          title='Observed Attributes'
          subtitle='attributes observed at this entity grain'
          color='green'
        />
        {entity.dimensions.length > 0 ? (
          <div className='flex flex-col gap-1'>
            {entity.dimensions.map((d) => (
              <DimensionRow
                key={d.name}
                dim={d}
                onSelect={() =>
                  onSelect({ kind: "dimension", entityId: entity.id, dimensionName: d.name })
                }
              />
            ))}
          </div>
        ) : (
          <span className='font-mono text-[10px] text-muted-foreground'>— no dimensions</span>
        )}
      </section>

      {/* Calculated attributes */}
      <section className='flex flex-col gap-1.5 border-border border-t pt-3'>
        <SectionHeader
          title='Calculated Attributes'
          subtitle='Σ_p of a source attribute along a promotion'
          color='violet'
        />
        {entity.own_measures.length === 0 && entity.induced_measures.length === 0 ? (
          <span className='font-mono text-[10px] text-muted-foreground'>
            — no calculated attributes
          </span>
        ) : (
          <div className='flex flex-col gap-1'>
            {entity.own_measures.map((m) => (
              <MeasureRow
                key={m.name}
                measure={m}
                induced={false}
                broken={false}
                onSelect={() =>
                  onSelect({
                    kind: "measure",
                    entityId: entity.id,
                    measureName: m.name,
                    induced: false
                  })
                }
              />
            ))}
            {entity.induced_measures.map((m) => (
              <MeasureRow
                key={`${m.name}:${m.promoted_from}`}
                measure={m}
                induced={true}
                broken={isInducedMeasureBroken(m, model.edges)}
                onSelect={() =>
                  onSelect({
                    kind: "measure",
                    entityId: entity.id,
                    measureName: m.name,
                    induced: true,
                    promotedFrom: m.promoted_from
                  })
                }
              />
            ))}
          </div>
        )}
      </section>
    </div>
  );
}

// ── Promotion body ────────────────────────────────────────────────────────────

/** Format a promotion as `p_{from→to}` */
function pLabel(from: string, to: string) {
  return `p_{${from}→${to}}`;
}

function FanoutWarning() {
  return (
    <div className='flex flex-col gap-1 border border-destructive/40 bg-destructive/5 px-3 py-2'>
      <div className='flex items-center gap-1.5 font-mono text-[10px] text-destructive uppercase tracking-wider'>
        <span>⇉</span>
        <span>fan-out promotion</span>
      </div>
      <p className='text-[11px] text-muted-foreground leading-snug'>
        This promotion is not a function — one fine-grain instance can map to multiple coarse-grain
        instances. Σ_p through a fan-out inflates values: each child is counted once{" "}
        <em>per parent it belongs to</em>, not once globally. Use{" "}
        <span className='font-mono text-foreground'>COUNT_DISTINCT</span> or restrict to a single
        parent to get accurate aggregates.
      </p>
    </div>
  );
}

function PromotionBody({
  from: fromId,
  to: toId,
  model,
  onSelect,
  instanceDetail = null
}: {
  from: string;
  to: string;
  model: WorldModel;
  onSelect: (s: WmSelection) => void;
  instanceDetail?: WmInstanceDetail | null;
}) {
  const fromEntity = model.entities.find((e) => e.id === fromId);
  const toEntity = model.entities.find((e) => e.id === toId);
  const edge = model.edges.find((e) => e.from === fromId && e.to === toId);
  const isFunctional = edge?.functional !== false;

  // Composed chains through this edge
  const composesBefore = model.edges.filter((e) => e.to === fromId);
  const composesAfter = model.edges.filter((e) => e.from === toId);

  const allForwardedMeasures = [
    ...(fromEntity?.own_measures ?? []).map((m) => ({ measure: m, induced: false })),
    ...(fromEntity?.induced_measures ?? []).map((m) => ({ measure: m, induced: true }))
  ];

  return (
    <div className='flex flex-col gap-3 p-3'>
      {/* Title */}
      <div className='min-w-0'>
        <h2 className='truncate font-medium text-[16px] text-foreground'>
          {fromId} → {toId}
        </h2>
        <div className='mt-0.5 font-mono text-[10px] uppercase tracking-wider'>
          <span className={isFunctional ? "text-success" : "text-destructive"}>
            {isFunctional ? "functional" : "fan-out ⇉"}
          </span>
        </div>
      </div>

      {/* Fan-out warning */}
      {!isFunctional && <FanoutWarning />}

      {/* Entity navigation */}
      <div className='flex flex-col gap-1'>
        {fromEntity && <EntityLink entity={fromEntity} onSelect={onSelect} />}
        <div className='flex items-center justify-center py-0.5 font-mono text-[10px] text-muted-foreground'>
          ↓ promotes to
        </div>
        {toEntity && <EntityLink entity={toEntity} onSelect={onSelect} />}
      </div>

      {/* Composition */}
      {(composesBefore.length > 0 || composesAfter.length > 0) && (
        <section className='flex flex-col gap-1.5 border-border border-t pt-3'>
          <SectionHeader title='Composition' subtitle='which promotions compose with this' />
          <div className='flex flex-col gap-1'>
            {composesBefore.map((e) => (
              <Row
                key={`before-${e.from}`}
                onClick={() => onSelect({ kind: "promotion", from: e.from, to: e.to })}
              >
                <span className='min-w-0 flex-1 truncate text-foreground'>
                  {pLabel(fromId, toId)}
                  <span className='text-muted-foreground'> ∘ </span>
                  {pLabel(e.from, e.to)}
                </span>
              </Row>
            ))}
            {composesAfter.map((e) => (
              <Row
                key={`after-${e.to}`}
                onClick={() => onSelect({ kind: "promotion", from: e.from, to: e.to })}
              >
                <span className='min-w-0 flex-1 truncate text-foreground'>
                  {pLabel(e.from, e.to)}
                  <span className='text-muted-foreground'> ∘ </span>
                  {pLabel(fromId, toId)}
                </span>
              </Row>
            ))}
          </div>
        </section>
      )}

      {/* Measures supported */}
      {allForwardedMeasures.length > 0 && (
        <section className='flex flex-col gap-1.5 border-border border-t pt-3'>
          <SectionHeader
            title='Measures Supported'
            subtitle='calculated attributes that pushforward through this promotion'
            color='violet'
          />
          <div className='flex flex-col gap-1'>
            {allForwardedMeasures.map(({ measure: m, induced }) => (
              <MeasureRow
                key={
                  induced
                    ? `${m.name}:${
                        // The array's element type collapses to the base measure, so `in`
                        // alone leaves this `unknown`; an induced measure's is a string.
                        "promoted_from" in m && typeof m.promoted_from === "string"
                          ? m.promoted_from
                          : ""
                      }`
                    : m.name
                }
                measure={m}
                induced={induced}
                onSelect={() =>
                  onSelect({
                    kind: "measure",
                    entityId: fromId,
                    measureName: m.name,
                    induced,
                    promotedFrom:
                      "promoted_from" in m
                        ? (m as WorldModelInducedMeasure).promoted_from
                        : undefined
                  })
                }
              />
            ))}
          </div>
        </section>
      )}

      {/* Fiber sample — shown when seed instance detail contains data for this promotion */}
      {(() => {
        const promotionLabel = `${fromId} → ${toId}`;
        const fiberSample = instanceDetail?.receives_from.find(
          (c) => c.promotion === promotionLabel
        );
        if (!fiberSample) return null;
        const shownSamples = fiberSample.sample.slice(0, 3);
        const remaining = fiberSample.fiber_count - shownSamples.length;
        return (
          <section className='flex flex-col gap-1.5 border-border border-t pt-3'>
            <SectionHeader
              title='Fiber Sample'
              subtitle={`|fiber| = ${fiberSample.fiber_count} at active instance`}
            />
            <div className='flex flex-col gap-1'>
              {shownSamples.map((label, i) => (
                <div
                  key={fiberSample.sample_keys[i] ?? label}
                  className='flex items-center gap-1.5 border border-border/60 bg-background/40 px-2 py-1 font-mono text-[10px]'
                >
                  <span className='text-info'>↓</span>
                  <span className='text-foreground'>{label}</span>
                </div>
              ))}
              {remaining > 0 && (
                <div className='pl-1 font-mono text-[9px] text-muted-foreground'>
                  + {remaining} more
                </div>
              )}
            </div>
          </section>
        );
      })()}
    </div>
  );
}

// ── Dimension body ────────────────────────────────────────────────────────────

function DimensionBody({
  entityId,
  dim,
  entity,
  onSelect
}: {
  entityId: string;
  dim: WorldModelDimension;
  entity: WorldModelEntity | undefined;
  onSelect: (s: WmSelection) => void;
}) {
  return (
    <div className='flex flex-col gap-3 p-3'>
      <div className='min-w-0'>
        <div className='flex items-baseline gap-2'>
          <span className='shrink-0 font-mono text-[14px] text-success leading-none'>●</span>
          <h2 className='truncate font-medium text-[16px] text-foreground'>
            {dim.label ?? dim.name}
          </h2>
        </div>
        <p className='mt-0.5 font-mono text-[10px] text-success uppercase tracking-wider'>
          observed · {dim.dim_type}
        </p>
      </div>

      <div className='border border-border bg-background/60 px-2 py-1.5 font-mono text-[11px] text-foreground'>
        {dim.name} : {entityId} → {dim.dim_type}
      </div>

      {dim.description && (
        <p className='break-words text-[11px] text-muted-foreground leading-snug'>
          {dim.description}
        </p>
      )}

      {entity && (
        <section className='flex flex-col gap-1.5 border-border border-t pt-3'>
          <SectionHeader title='Carrier Entity' />
          <EntityLink entity={entity} onSelect={onSelect} />
        </section>
      )}
    </div>
  );
}

// ── Measure body ─────────────────────────────────────────────────────────────

function MeasureBody({
  entityId,
  measureName,
  induced,
  promotedFrom,
  measure,
  entity,
  model,
  onSelect
}: {
  entityId: string;
  measureName: string;
  induced: boolean;
  promotedFrom?: string;
  measure: WorldModelMeasure | WorldModelInducedMeasure | undefined;
  entity: WorldModelEntity | undefined;
  model: WorldModel;
  onSelect: (s: WmSelection) => void;
}) {
  const additivity = measure?.additivity ?? "passthrough";
  const measureType = measure?.measure_type ?? "";

  return (
    <div className='flex flex-col gap-3 p-3'>
      <div className='min-w-0'>
        <div className='flex items-baseline gap-2'>
          <span
            className={cn(
              "shrink-0 font-mono text-[16px] leading-none",
              measureSymbolColor(measureType)
            )}
          >
            {measureSymbol(measureType)}
          </span>
          <h2 className='truncate font-medium text-[16px] text-foreground'>{measureName}</h2>
        </div>
        <p className='mt-0.5 font-mono text-[10px] text-[color:var(--vis-purple)] uppercase tracking-wider'>
          {induced ? "induced · " : ""}
          {measureType} · {additivity.replace("_", "-")}
        </p>
      </div>

      {/* Definition */}
      <section className='flex flex-col gap-1.5 border-border border-t pt-3'>
        {induced ? (
          <>
            <SectionHeader
              title='Definition'
              subtitle='Σ_p of a source attribute along a promotion'
            />
            <div className='border border-border bg-background/60 px-2 py-1.5 font-mono text-xs'>
              <div className='flex flex-col gap-1 text-muted-foreground'>
                <div>
                  <span className='text-foreground'>{measureName}</span>
                  {" = "}
                  <span className={measureSymbolColor(measureType)}>
                    {measureSymbol(measureType)}
                  </span>
                  {"(x ∈ p⁻¹(·)) "}
                  <span>{measureName}(x)</span>
                </div>
                <div>
                  operator:{" "}
                  <span className={measureSymbolColor(measureType)}>
                    {measureSymbol(measureType)}
                  </span>
                  {" · "}
                  {additivity === "additive" && (
                    <span className='text-success'>fully additive</span>
                  )}
                  {additivity === "non_additive" && (
                    <span className='text-status-warning'>non-additive</span>
                  )}
                  {additivity === "passthrough" && <span>passthrough</span>}
                </div>
              </div>
            </div>
            {promotedFrom && (
              <div className='font-mono text-[10px] text-muted-foreground'>
                declared on <span className='text-foreground'>{promotedFrom}</span>, promoted to{" "}
                <span className='text-foreground'>{entityId}</span>
              </div>
            )}
          </>
        ) : (
          <>
            <SectionHeader title='Definition' />
            <div className='border border-border bg-background/60 px-2 py-1.5 font-mono text-xs'>
              <div className='flex flex-col gap-1 text-muted-foreground'>
                {measure?.expr && (
                  <div>
                    <span className={measureSymbolColor(measureType)}>
                      {measureSymbol(measureType)}
                    </span>
                    {"("}
                    <span className='text-foreground'>{measure.expr}</span>
                    {")"}
                  </div>
                )}
                <div>
                  operator:{" "}
                  <span className={measureSymbolColor(measureType)}>
                    {measureSymbol(measureType)}
                  </span>
                  {" · "}
                  {additivity === "additive" && (
                    <span className='text-success'>fully additive</span>
                  )}
                  {additivity === "non_additive" && (
                    <span className='text-status-warning'>non-additive</span>
                  )}
                  {additivity === "passthrough" && <span>passthrough</span>}
                </div>
              </div>
            </div>
          </>
        )}
      </section>

      {measure?.description && (
        <p className='break-words text-[11px] text-muted-foreground leading-snug'>
          {measure.description}
        </p>
      )}

      {entity && (
        <section className='flex flex-col gap-1.5 border-border border-t pt-3'>
          <SectionHeader title='Host Entity' subtitle="this measure's home grain" />
          <EntityLink entity={entity} onSelect={onSelect} />
        </section>
      )}

      <WorldModelMeasureAnalysis
        measureName={measureName}
        induced={induced}
        promotedFrom={promotedFrom}
        entityView={entity?.view}
        additivity={additivity}
        model={model}
        onSelect={onSelect}
      />
    </div>
  );
}

// ── Shared row components ─────────────────────────────────────────────────────

function PromotionRow({
  edge,
  label,
  direction,
  onSelect
}: {
  edge: WorldModelEdge;
  label: string;
  direction: "in" | "out";
  onSelect: (s: WmSelection) => void;
}) {
  const fanout = edge.functional === false;
  return (
    <button
      type='button'
      className={cn(
        "flex w-full items-center gap-2 border bg-background/40 px-2 py-1 text-left text-[11px] hover:border-info/60",
        fanout ? "border-destructive/50" : "border-border"
      )}
      onClick={() => onSelect({ kind: "promotion", from: edge.from, to: edge.to })}
    >
      <span className='shrink-0 text-info'>{direction === "in" ? "↓" : "↑"}</span>
      <span className='min-w-0 flex-1 truncate text-foreground'>{label}</span>
      <span
        className={cn(
          "ml-auto shrink-0 font-mono text-[9px]",
          fanout ? "text-destructive" : "text-muted-foreground"
        )}
      >
        {fanout ? "⇉ fanout" : "function"}
      </span>
    </button>
  );
}

function DimensionRow({ dim, onSelect }: { dim: WorldModelDimension; onSelect: () => void }) {
  return (
    <button
      type='button'
      className='flex w-full items-baseline gap-2 border border-border bg-background/40 px-2 py-1 text-left text-[11px] hover:border-success/60'
      onClick={onSelect}
    >
      <span className='shrink-0 font-mono text-[12px] text-success leading-none'>●</span>
      <span className='truncate text-foreground'>{dim.label ?? dim.name}</span>
      <span className='shrink-0 font-mono text-[9px] text-muted-foreground'>: {dim.dim_type}</span>
    </button>
  );
}

function measureFormula(
  measure: WorldModelMeasure | WorldModelInducedMeasure,
  induced: boolean
): string {
  const sym = measureSymbol(measure.measure_type);
  const exprStr =
    measure.expr ??
    (measure.measure_type === "count" || measure.measure_type === "count_distinct" ? "1" : "·");
  const base = `${sym}(${exprStr})`;
  if (induced && "path" in measure) {
    const chain = (measure as WorldModelInducedMeasure).path;
    return chain.length > 0 ? `${base} ↗ ${chain.join(" ∘ ")}` : base;
  }
  return base;
}

function MeasureRow({
  measure,
  induced,
  broken = false,
  onSelect
}: {
  measure: WorldModelMeasure | WorldModelInducedMeasure;
  induced: boolean;
  broken?: boolean;
  onSelect: () => void;
}) {
  const formula = measureFormula(measure, induced);
  return (
    <button
      type='button'
      className={cn(
        "flex w-full min-w-0 cursor-pointer flex-col bg-background/40 px-2 py-1.5 font-mono text-xs transition-colors hover:border-info/60",
        induced ? "border border-l-2 border-l-[color:var(--vis-purple)]" : "border border-border",
        broken && "border-destructive/50"
      )}
      onClick={onSelect}
    >
      {/* Row 1: symbol + name + type + broken badge */}
      <div className='flex min-w-0 items-baseline gap-1.5'>
        <span
          className={cn(
            "w-3 shrink-0 text-center font-mono text-[11px] leading-none",
            measureSymbolColor(measure.measure_type)
          )}
        >
          {measureSymbol(measure.measure_type)}
        </span>
        <span className='truncate text-foreground'>{measure.label ?? measure.name}</span>
        <span className='shrink-0 font-mono text-[9px] text-muted-foreground'>
          : {measure.measure_type}
        </span>
        {broken && (
          <span
            className='shrink-0 font-mono text-[9px] text-destructive'
            title='fan-out promotion in path — Σ_p may double-count'
          >
            ⇉
          </span>
        )}
      </div>
      {/* Row 2: formula + promotion chain */}
      <div className='mt-0.5 truncate pl-4.5 text-[9.5px] text-muted-foreground'>{formula}</div>
    </button>
  );
}

// ── Computed measure row (instance body) ───────────────────────────────────────

function ComputedMeasureRow({
  measure: m,
  def,
  entityId,
  entityView,
  keyValue,
  model,
  onSelect,
  expandedEntityId,
  breakdownMeasure,
  onToggle
}: {
  measure: WmComputedMeasure;
  def: WorldModelMeasure | WorldModelInducedMeasure | undefined;
  entityId: string;
  entityView: string | undefined;
  keyValue: string;
  model: WorldModel;
  onSelect: (s: WmSelection) => void;
  expandedEntityId?: string | null;
  breakdownMeasure?: string | null;
  onToggle?: (measure: string | null) => void;
}) {
  // Driven by the same expandedEntityId/breakdownMeasure state as the graph
  // node, so expanding here expands the node and vice versa.
  const expanded = entityId === expandedEntityId && m.name === breakdownMeasure;
  const isNonAdditive = def?.additivity === "non_additive";
  const hasBreakdown = def?.has_breakdown === true;
  // An induced measure is promoted to this grain from another view; opportunity
  // sizing must address it on that declaring view, so surface the promotion.
  const induced = !!def && "promoted_from" in def;
  const promotedFrom = induced ? (def as WorldModelInducedMeasure).promoted_from : undefined;
  const exprStr =
    def?.expr ?? (m.measure_type === "count" || m.measure_type === "count_distinct" ? "1" : "·");
  // An empty fiber means this instance has no underlying rows for the measure,
  // so there is nothing here to size — hide the opportunities/segment-spread
  // toggle rather than offer a population benchmark against data this instance
  // doesn't have. `value === null` is the still-loading skeleton (fiber_count is
  // seeded 0 then), so only suppress once the measure has actually resolved.
  const hasData = m.value !== null && m.fiber_count > 0;

  return (
    <div className='flex flex-col border border-border bg-background/40 px-2 py-1.5'>
      <div className='flex min-w-0 items-baseline justify-between gap-2'>
        <div className='flex min-w-0 shrink items-baseline gap-1.5 font-mono text-xs'>
          {hasBreakdown ? (
            <button
              type='button'
              className='-ml-0.5 shrink-0 self-center text-info transition-colors hover:text-foreground'
              title={expanded ? "Hide breakdown" : "Break down at this instance"}
              data-testid={`wm-panel-measure-zoom-${entityId}-${m.name}`}
              onClick={() => onToggle?.(expanded ? null : m.name)}
            >
              {expanded ? <ChevronDown className='size-3' /> : <ChevronRight className='size-3' />}
            </button>
          ) : (
            <span
              className={cn(
                "shrink-0 font-mono text-[11px] leading-none",
                measureSymbolColor(m.measure_type)
              )}
            >
              {measureSymbol(m.measure_type)}
            </span>
          )}
          <span className='truncate text-foreground'>{m.label ?? m.name}</span>
          <span className='shrink-0 font-mono text-[9px] text-muted-foreground'>
            : {m.measure_type}
          </span>
          {isNonAdditive && (
            <span
              className='shrink-0 font-mono text-[9px] text-status-warning'
              title='non-additive: cannot be safely re-aggregated'
            >
              ⚠
            </span>
          )}
        </div>
        {m.value === null ? (
          <span className='h-3 w-14 animate-pulse bg-muted' />
        ) : (
          <span className='shrink-0 font-mono text-[12px] text-info tabular-nums' title={m.value}>
            {formatMeasureValue(m.value)}
          </span>
        )}
      </div>
      <div className='mt-0.5 truncate pl-4 font-mono text-[9.5px] text-muted-foreground'>
        {m.value === null ? (
          <span className='inline-block h-2 w-24 animate-pulse bg-muted' />
        ) : (
          `${measureSymbol(m.measure_type)}(${exprStr}) · |fiber|=${m.fiber_count}`
        )}
      </div>
      {expanded && hasBreakdown && (
        <div className='mt-2 border-border/60 border-t pt-2'>
          <WorldModelDriverTreeLive entityId={entityId} keyValue={keyValue} measure={m.name} />
        </div>
      )}
      {def && hasData && (
        <WorldModelInstanceOpportunity
          measureName={m.name}
          induced={induced}
          promotedFrom={promotedFrom}
          entityView={entityView}
          additivity={def.additivity}
          instance={{ entity: entityId, key: keyValue }}
          model={model}
          onSelect={onSelect}
        />
      )}
    </div>
  );
}

// ── Instance body ─────────────────────────────────────────────────────────────

function InstanceBody({
  entityId,
  keyValue,
  label,
  model,
  onSelect,
  expandedEntityId,
  breakdownMeasure,
  onExpandMeasure
}: {
  entityId: string;
  keyValue: string;
  label: string;
  model: WorldModel;
  onSelect: (s: WmSelection) => void;
  expandedEntityId?: string | null;
  breakdownMeasure?: string | null;
  onExpandMeasure?: (
    entityId: string,
    keyValue: string,
    label: string,
    measure: string | null
  ) => void;
}) {
  const { data, isLoading, error } = useWmInstanceDetail(entityId, keyValue);

  const entity = model.entities.find((e) => e.id === entityId);

  // A section is "pending" (show spinner) when:
  //   - data hasn't arrived yet (waiting for first init event), OR
  //   - streaming is still in progress and this section's array is still empty
  // A section is "done-empty" (hide) when !isLoading and its array is empty.
  // After a failed stream nothing more arrives, so no section is pending.
  const pending = (arr: unknown[] | undefined) =>
    !error && (!data || (isLoading && (arr ?? []).length === 0));

  return (
    <div className='flex flex-col gap-3 p-3'>
      {/* Title */}
      <div className='min-w-0'>
        <h2 className='truncate font-medium text-[16px] text-foreground'>
          {data?.display ?? label}
        </h2>
        <p className='mt-0.5 font-mono text-[10px] text-info uppercase tracking-wider'>
          in {entityId}
        </p>
      </div>

      {/* Jump to entity */}
      {entity && <EntityLink entity={entity} onSelect={onSelect} />}

      {error && (
        <p className='border-border border-t pt-3 font-mono text-destructive text-xs'>
          Failed to load this instance's details.
        </p>
      )}

      {/* Attribute values */}
      {(pending(data?.attributes) || (data?.attributes.length ?? 0) > 0) && (
        <section className='flex flex-col gap-1.5 border-border border-t pt-3'>
          <SectionHeader title='Attribute Values' subtitle='observed data on this instance' />
          {pending(data?.attributes) ? (
            <SectionSpinner />
          ) : (
            <div className='border border-border'>
              {data?.attributes.map((attr) => (
                <div
                  key={attr.name}
                  className='flex items-baseline justify-between border-border/30 border-b px-2 py-1 font-mono text-xs last:border-0'
                >
                  <span className='text-[9px] text-muted-foreground uppercase tracking-wider'>
                    {attr.label ?? attr.name}
                  </span>
                  <span className='ml-4 truncate text-right text-foreground tabular-nums'>
                    {attr.value || "—"}
                  </span>
                </div>
              ))}
            </div>
          )}
        </section>
      )}

      {/* Promotes to */}
      {(pending(data?.promotes_to) || (data?.promotes_to.length ?? 0) > 0) && (
        <section className='flex flex-col gap-1.5 border-border border-t pt-3'>
          <SectionHeader
            title='Promotes To'
            subtitle='outbound — parent instance(s) per promotion'
          />
          {pending(data?.promotes_to) ? (
            <SectionSpinner />
          ) : (
            <div className='flex flex-col gap-2'>
              {data?.promotes_to.map((parent) => (
                <div key={parent.promotion} className='flex flex-col gap-1'>
                  <span className='font-mono text-[10px] text-muted-foreground'>
                    {parent.promotion}
                  </span>
                  <button
                    type='button'
                    className='flex items-center gap-1.5 border border-info/30 bg-background/60 px-2 py-1 text-left font-mono text-[10.5px] transition-colors hover:border-info/70'
                    onClick={() =>
                      onSelect({
                        kind: "instance",
                        entityId: parent.promotion.split(" → ")[1] ?? entityId,
                        keyValue: parent.key,
                        label: parent.display
                      })
                    }
                  >
                    <span className='text-info'>↑</span>
                    <span className='truncate text-foreground'>{parent.display}</span>
                    <span className='ml-auto shrink-0 font-mono text-[9px] text-muted-foreground'>
                      {parent.key}
                    </span>
                  </button>
                </div>
              ))}
            </div>
          )}
        </section>
      )}

      {/* Receives from */}
      {(pending(data?.receives_from) || (data?.receives_from.length ?? 0) > 0) && (
        <section className='flex flex-col gap-1.5 border-border border-t pt-3'>
          <SectionHeader
            title='Receives From'
            subtitle='inbound fibers — bag of children at finer grains'
          />
          {pending(data?.receives_from) ? (
            <SectionSpinner />
          ) : (
            <div className='flex flex-col gap-3'>
              {data?.receives_from.map((child) => (
                <div key={child.promotion} className='flex flex-col gap-1.5'>
                  <div className='flex items-baseline justify-between font-mono text-[10px]'>
                    <span className='text-muted-foreground'>{child.promotion}</span>
                    <span className='text-muted-foreground'>|fiber|={child.fiber_count}</span>
                  </div>
                  <div className='flex flex-wrap gap-1'>
                    {child.sample.map((display, i) => (
                      <button
                        key={child.sample_keys[i] ?? display}
                        type='button'
                        className='flex items-center gap-1 border border-border bg-background/60 px-1.5 py-0.5 font-mono text-[10px] transition-colors hover:border-info/60'
                        onClick={() =>
                          onSelect({
                            kind: "instance",
                            entityId: child.promotion.split(" → ")[0] ?? entityId,
                            keyValue: child.sample_keys[i] ?? display,
                            label: display
                          })
                        }
                      >
                        <span className='text-info'>↓</span>
                        <span className='text-foreground'>{display}</span>
                      </button>
                    ))}
                  </div>
                </div>
              ))}
            </div>
          )}
        </section>
      )}

      {/* Computed measures */}
      {(pending(data?.computed_measures) || (data?.computed_measures.length ?? 0) > 0) && (
        <section className='flex flex-col gap-1.5 border-border border-t pt-3'>
          <SectionHeader
            title='Computed Measures'
            subtitle="Σ_p evaluated over this instance's real fiber"
            color='violet'
          />
          {pending(data?.computed_measures) ? (
            <SectionSpinner />
          ) : (
            <div className='flex flex-col gap-1'>
              {data?.computed_measures.map((m) => {
                const def =
                  entity?.own_measures.find((d) => d.name === m.name) ??
                  entity?.induced_measures.find((d) => d.name === m.name);
                return (
                  <ComputedMeasureRow
                    key={m.name}
                    measure={m}
                    def={def}
                    entityId={entityId}
                    entityView={entity?.view}
                    keyValue={keyValue}
                    model={model}
                    onSelect={onSelect}
                    expandedEntityId={expandedEntityId}
                    breakdownMeasure={breakdownMeasure}
                    onToggle={(measure) => onExpandMeasure?.(entityId, keyValue, label, measure)}
                  />
                );
              })}
            </div>
          )}
        </section>
      )}
    </div>
  );
}

// ── Root ─────────────────────────────────────────────────────────────────────

interface WorldModelDetailPanelProps {
  model: WorldModel;
  selection: WmSelection;
  onSelect: (s: WmSelection) => void;
  history: WmSelection[];
  onBack: () => void;
  seedInstanceDetail?: WmInstanceDetail | null;
  /** Mirrors the graph's own expansion state so a measure expanded here
   *  expands the matching node, and vice versa. */
  expandedEntityId?: string | null;
  breakdownMeasure?: string | null;
  onExpandMeasure?: (
    entityId: string,
    keyValue: string,
    label: string,
    measure: string | null
  ) => void;
}

export function WorldModelDetailPanel({
  model,
  selection,
  onSelect,
  history,
  onBack,
  seedInstanceDetail = null,
  expandedEntityId = null,
  breakdownMeasure = null,
  onExpandMeasure
}: WorldModelDetailPanelProps) {
  if (!selection) {
    return (
      <div className='flex h-full items-center justify-center p-4 text-center font-mono text-muted-foreground text-xs'>
        Click an entity node to explore its attributes and promotion chain.
      </div>
    );
  }

  const breadcrumb = (() => {
    switch (selection.kind) {
      case "entity":
        return `Entity · ${selection.entityId}`;
      case "promotion":
        return `Promotion · p_{${selection.from}→${selection.to}}`;
      case "dimension":
        return `Attribute · ${selection.entityId} · ${selection.dimensionName}`;
      case "measure":
        return `Measure · ${selection.entityId} · ${selection.measureName}`;
      case "instance":
        return `Instance · ${selection.entityId} · ${selection.label}`;
    }
  })();

  const canGoBack = history.length > 0;

  const renderBody = () => {
    switch (selection.kind) {
      case "entity": {
        const entity = model.entities.find((e) => e.id === selection.entityId);
        if (!entity) return null;
        return <EntityBody entity={entity} model={model} onSelect={onSelect} />;
      }
      case "promotion":
        return (
          <PromotionBody
            from={selection.from}
            to={selection.to}
            model={model}
            onSelect={onSelect}
            instanceDetail={seedInstanceDetail}
          />
        );
      case "dimension": {
        const entity = model.entities.find((e) => e.id === selection.entityId);
        const dim = entity?.dimensions.find((d) => d.name === selection.dimensionName);
        if (!dim) return null;
        return (
          <DimensionBody
            entityId={selection.entityId}
            dim={dim}
            entity={entity}
            onSelect={onSelect}
          />
        );
      }
      case "measure": {
        const entity = model.entities.find((e) => e.id === selection.entityId);
        const measure = selection.induced
          ? entity?.induced_measures.find((m) => m.name === selection.measureName)
          : entity?.own_measures.find((m) => m.name === selection.measureName);
        return (
          <MeasureBody
            entityId={selection.entityId}
            measureName={selection.measureName}
            induced={selection.induced}
            promotedFrom={selection.promotedFrom}
            measure={measure}
            entity={entity}
            model={model}
            onSelect={onSelect}
          />
        );
      }
      case "instance":
        return (
          <InstanceBody
            entityId={selection.entityId}
            keyValue={selection.keyValue}
            label={selection.label}
            model={model}
            onSelect={onSelect}
            expandedEntityId={expandedEntityId}
            breakdownMeasure={breakdownMeasure}
            onExpandMeasure={onExpandMeasure}
          />
        );
    }
  };

  return (
    <div className='flex h-full flex-col overflow-hidden'>
      <PanelHeader
        breadcrumb={breadcrumb}
        onBack={canGoBack ? onBack : undefined}
        onClose={() => onSelect(null)}
      />
      <div className='min-h-0 flex-1 overflow-y-auto'>{renderBody()}</div>
    </div>
  );
}
