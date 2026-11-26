import { ChevronDown, ChevronRight } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { useProjection } from "@/hooks/api/useMetricTree";
import type { ProjectionGranularity } from "@/types/metricTree";
import { SectionHeader } from "../../components/semanticGraph";
import type { ScenarioNodeData } from "./nodeValue";
import { ProjectionBody } from "./ProjectionBody";
import { scenarioCurve } from "./projectionCurves";
import {
  buildProjectionRequest,
  defaultHorizon,
  GRANULARITIES,
  horizonChoices
} from "./projectionRequest";
import { type ProjectionTarget, projectionTargets, resolveTarget } from "./projectionTargets";
import type { ScenarioState } from "./scenarioUrl";

interface ProjectionPanelProps {
  state: ScenarioState;
  nodeData: Map<string, ScenarioNodeData>;
  /** The canvas selection, which seeds the measure picker. */
  selectedId: string | null;
  /** A conflicting lever set spends no query here either. */
  blocked: boolean;
}

/**
 * Forward projection for one measure: the baseline curve from the warehouse,
 * the scenario curve composed on top of it.
 *
 * **Collapsed by default, and that is load-bearing.** The curve is a second
 * warehouse query over a window several times longer than the scenario's own,
 * so a scenario nobody asked to project must not pay for one.
 *
 * The charted measure is an explicit dropdown rather than "whatever is selected
 * on the canvas". Selection still drives it — clicking a node moves the picker
 * — but a chart that silently re-targets with no visible control leaves the
 * analyst with no way to ask for the measure they actually want.
 */
export function ProjectionPanel({ state, nodeData, selectedId, blocked }: ProjectionPanelProps) {
  const [open, setOpen] = useState(false);
  const [granularity, setGranularity] = useState<ProjectionGranularity>("day");
  const [horizon, setHorizon] = useState<number>(() => defaultHorizon("day"));
  const [chosen, setChosen] = useState<string | null>(null);

  const targets = useMemo(() => projectionTargets(nodeData), [nodeData]);
  const measureId = resolveTarget(targets, chosen);

  // Clicking a node on the canvas moves the picker, but only to something the
  // picker actually offers: the canvas is also how the next lever gets found,
  // and hunting for one must not blank the chart on every click.
  //
  // The ref is what makes this fire on a *change* of selection rather than on
  // every render of the effect. `targets` is a memo over `nodeData`, which is a
  // fresh `Map` on every `predict` response — so the honest-looking dep list
  // (`[selectedId, targets]`) re-runs the sync roughly 300ms after every lever
  // nudge and overwrites whatever the analyst had picked from the dropdown.
  // Narrowing the guard to "something the picker offers" does not help: the
  // selected node is offered, which is precisely why the sync runs. Only the
  // ref can tell "the canvas moved" from "the identity of `targets` moved".
  // Reading `targets` without depending on it is the point, hence the disabled
  // rule rather than a wider dep list.
  //
  // Nothing is lost by not re-running: a chosen measure that later drops out of
  // the targets still falls back to a valid one, in `resolveTarget` above.
  const syncedSelection = useRef<string | null>(null);
  // biome-ignore lint/correctness/useExhaustiveDependencies: `targets` is read, deliberately not depended on — see above.
  useEffect(() => {
    if (selectedId === syncedSelection.current) return;
    syncedSelection.current = selectedId;
    if (selectedId && targets.some((t) => t.nodeId === selectedId)) setChosen(selectedId);
  }, [selectedId]);

  const request = useMemo(
    () => (open ? buildProjectionRequest(blocked, state, granularity, horizon) : null),
    [open, blocked, state, granularity, horizon]
  );
  const { data, isFetching, error } = useProjection(request);

  const projection = data?.series.find((s) => s.measure === measureId);
  const node = measureId ? nodeData.get(measureId) : undefined;

  const curve = useMemo(
    () =>
      projection
        ? scenarioCurve({
            projection,
            baselineValue: node?.baseline,
            delta: node?.delta,
            confidence: node?.confidence,
            lagDays: node?.lag
          })
        : null,
    [projection, node]
  );

  return (
    <div
      className='flex flex-col gap-2 border-border border-t p-4'
      data-testid='metric-tree-projection-panel'
    >
      <button
        type='button'
        className='flex w-full items-center gap-1.5 text-left'
        onClick={() => setOpen((v) => !v)}
        data-testid='metric-tree-projection-toggle'
      >
        {open ? (
          <ChevronDown className='size-3 shrink-0 text-muted-foreground' />
        ) : (
          <ChevronRight className='size-3 shrink-0 text-muted-foreground' />
        )}
        <span className='min-w-0 flex-1'>
          <SectionHeader
            title='Project forward'
            subtitle={open ? undefined : `${horizon} ${granularity}s`}
          />
        </span>
      </button>

      {open && (
        <>
          <Controls
            targets={targets}
            measureId={measureId}
            granularity={granularity}
            horizon={horizon}
            onMeasure={setChosen}
            onGranularity={(next) => {
              setGranularity(next);
              // The horizon is counted in buckets, so 30 means a month of days
              // and half a year of weeks. Carrying the number across would
              // silently change what was asked for.
              setHorizon(defaultHorizon(next));
            }}
            onHorizon={setHorizon}
          />
          <ProjectionBody
            note={note({ request, error, data: data?.projection_note, projection })}
            projection={projection}
            curve={curve}
            isFetching={isFetching}
          />
        </>
      )}
    </div>
  );
}

interface ControlsProps {
  targets: ProjectionTarget[];
  measureId: string | null;
  granularity: ProjectionGranularity;
  horizon: number;
  onMeasure: (next: string) => void;
  onGranularity: (next: ProjectionGranularity) => void;
  onHorizon: (next: number) => void;
}

/** Measure on its own row — it is the question being asked, and its name is the
 *  longest string here. Granularity and horizon share the row below. */
function Controls({
  targets,
  measureId,
  granularity,
  horizon,
  onMeasure,
  onGranularity,
  onHorizon
}: ControlsProps) {
  return (
    <div className='flex flex-col gap-1.5'>
      <Select value={measureId ?? ""} onValueChange={onMeasure} disabled={targets.length === 0}>
        <SelectTrigger
          size='sm'
          aria-label='Measure to project'
          className='h-7 w-full font-mono text-xs'
          data-testid='projection-measure'
        >
          <SelectValue placeholder='no measure moved' />
        </SelectTrigger>
        <SelectContent className={DESIGN_REFERENCE}>
          {targets.map((target) => (
            <SelectItem key={target.nodeId} value={target.nodeId} className='font-mono text-xs'>
              {target.label}
              {target.isLever && <span className='ml-1.5 text-muted-foreground'>lever</span>}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      <div className='flex items-center gap-1.5'>
        <Select
          value={granularity}
          onValueChange={(v) => onGranularity(v as ProjectionGranularity)}
        >
          <SelectTrigger
            size='sm'
            aria-label='Bucket'
            className='h-7 flex-1 font-mono text-xs'
            data-testid='projection-granularity'
          >
            <SelectValue />
          </SelectTrigger>
          <SelectContent className={DESIGN_REFERENCE}>
            {GRANULARITIES.map((g) => (
              <SelectItem key={g} value={g} className='font-mono text-xs'>
                {g}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Select value={String(horizon)} onValueChange={(v) => onHorizon(Number(v))}>
          <SelectTrigger
            size='sm'
            aria-label='Horizon'
            className='h-7 flex-1 font-mono text-xs'
            data-testid='projection-horizon'
          >
            <SelectValue />
          </SelectTrigger>
          <SelectContent className={DESIGN_REFERENCE}>
            {horizonChoices(granularity).map((n) => (
              <SelectItem key={n} value={String(n)} className='font-mono text-xs'>
                {n} {granularity}
                {n === 1 ? "" : "s"} ahead
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
    </div>
  );
}

interface NoteArgs {
  request: unknown;
  error: Error | null;
  data: string | null | undefined;
  projection: { refusal?: string | null } | undefined;
}

/**
 * The one sentence to show instead of, or under, the chart.
 *
 * Ordered by how far the request got, so the message names the thing that
 * actually stopped it: no request at all, then a failed one, then a query that
 * ran and found nothing, then a series that could not be fitted.
 */
function note({ request, error, data, projection }: NoteArgs): string | null {
  if (!request) return "pin a lever and pick a time dimension to project forward";
  if (error) return error.message;
  if (data) return data;
  return projection?.refusal ?? null;
}
