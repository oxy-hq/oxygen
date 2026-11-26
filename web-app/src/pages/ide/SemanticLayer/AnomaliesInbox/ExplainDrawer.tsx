import { ArrowUp, ChevronDown, ChevronRight, RefreshCw } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Button } from "@/components/ui/shadcn/button";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle
} from "@/components/ui/shadcn/sheet";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/shadcn/tabs";
import { Textarea } from "@/components/ui/shadcn/textarea";
import { useAnomalyExplain, useRefreshAnomalyExplain } from "@/hooks/api/useMetricAnomalies";
import ROUTES from "@/libs/utils/routes";
import { ExplainNodeRow } from "@/pages/ide/MetricTree/components/ExplainTree";
import useCurrentOrg from "@/stores/useCurrentOrg";
import type { MetricAnomaly } from "@/types/metricAnomalies";
import type { DriverAttribution, ExplainResult, PassthroughSplit } from "@/types/metricTree";
import { formatNumber, formatPercent, formatSigned, shortMeasureName } from "@/utils/measureFormat";
import { groupDrivers } from "./driverClassification";
import ExplainGraph from "./ExplainGraph";
import { buildFollowUpPrompt, type DerivedPeriods, warningMessage } from "./followUpPrompt";

interface Props {
  anomaly: MetricAnomaly | null;
  /** Called when the user closes the drawer (X, escape, or outside click). */
  onOpenChange: (open: boolean) => void;
}

/**
 * Side drawer that calls `POST /semantic/metric-tree/explain` directly for
 * the given anomaly and renders the decomposition tree. Skips the analytics
 * agent entirely — deterministic, fast, no LLM ambiguity.
 *
 * Periods are derived from the anomaly:
 *   current  = (period_start, period_start)         — single bucket
 *   previous = (period_start - 1 season, same)      — for the period-over-period
 *
 * The same-cycle-back baseline lines up with the detector's `seasonality`
 * config, so the agent's explain compares the anomaly bucket against the
 * matching prior bucket (e.g. Sat vs Sat for daily / weekly seasonality).
 *
 * A "Ask a follow-up" button opens the home chat with the explain result
 * summarized so the agent can answer follow-up questions with full context.
 */
export default function ExplainDrawer({ anomaly, onOpenChange }: Props) {
  const navigate = useNavigate();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const [followUp, setFollowUp] = useState("");

  const periods = useMemo(() => (anomaly ? deriveExplainPeriods(anomaly) : null), [anomaly]);

  // Server-cached explain. The first time we hit `/explain` for this
  // anomaly, airlayer runs the recursive search and the result is
  // written onto the row's `explain_cache` JSONB column. Every later
  // call — including after a page refresh — returns the cached payload
  // instantly. The cache lifecycle is tied to the anomaly row.
  const explain = useAnomalyExplain(anomaly?.id ?? null);
  const refresh = useRefreshAnomalyExplain();

  // Clear the follow-up textarea whenever the user switches anomalies.
  // `anomaly?.id` is the discriminator — same id = same row = keep draft.
  // biome-ignore lint/correctness/useExhaustiveDependencies: intentional, only react to row id changes
  useEffect(() => {
    setFollowUp("");
  }, [anomaly?.id]);

  if (!anomaly) return null;

  const homePath = ROUTES.ORG(orgSlug).WORKSPACE().HOME;
  const canSubmit = !!explain.data && followUp.trim().length > 0;

  const submitFollowUp = () => {
    if (!canSubmit) return;
    navigate(homePath, {
      state: {
        prefillQuestion: buildFollowUpPrompt(
          anomaly,
          periods,
          explain.data ?? null,
          followUp.trim()
        ),
        autoSubmit: true
      }
    });
    onOpenChange(false);
  };

  /** Plain Enter submits (matches the rest of the app's MessageInput).
   *  Shift+Enter inserts a newline. */
  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      submitFollowUp();
    }
  };

  return (
    <Sheet open={!!anomaly} onOpenChange={onOpenChange}>
      {/* gap-0 + p-0 strip the default SheetContent spacing — we own the
          internal stack so the body, observed-vs-expected card, and footer
          line up edge-to-edge without the double-padding the defaults give. */}
      <SheetContent
        side='right'
        className={`${DESIGN_REFERENCE} flex w-full flex-col gap-0 p-0 sm:max-w-xl`}
      >
        <SheetHeader className='relative border-border border-b pb-3'>
          {/* Refresh: bare icon button to match SheetContent's own X
              close — which sits at `top-4 right-4` and is just a 16px
              icon with opacity-70. Placing ours at `top-4 right-12`
              with the same shape keeps both controls visually aligned. */}
          {explain.data && (
            <button
              type='button'
              onClick={() => refresh.mutate(anomaly.id)}
              disabled={refresh.isPending}
              aria-label='Refresh explain'
              className='absolute top-4 right-12 rounded-xs opacity-70 transition-opacity hover:opacity-100 disabled:pointer-events-none disabled:opacity-50'
            >
              <RefreshCw className={refresh.isPending ? "size-4 animate-spin" : "size-4"} />
            </button>
          )}
          <SheetTitle className='pr-20'>{anomaly.label || anomaly.measure}</SheetTitle>
          <SheetDescription className='t-code pr-20 text-xs'>
            {anomaly.measure} · {anomaly.period_start.slice(0, 10)} · {anomaly.granularity}
          </SheetDescription>
        </SheetHeader>

        <div className='flex flex-1 flex-col gap-3 overflow-hidden p-4'>
          <ObservedVsExpected anomaly={anomaly} />
          {explain.isPending && (
            <div className='flex h-32 items-center justify-center'>
              <Spinner />
            </div>
          )}
          {explain.error && (
            <p className='text-destructive text-sm'>
              {explain.error instanceof Error
                ? explain.error.message
                : "Failed to run the explain decomposition."}
            </p>
          )}
          {explain.data && <ExplainBody result={explain.data} />}
        </div>

        {/* Compose a follow-up. Submit ships <explain context> + <user
            question> to the home chat so the agent answers with full
            anomaly + decomposition context, not from scratch. */}
        <SheetFooter className='gap-1.5 border-border border-t'>
          <div className='relative'>
            <Textarea
              value={followUp}
              onChange={(e) => setFollowUp(e.target.value)}
              onKeyDown={onKeyDown}
              placeholder='Ask a follow-up — the explain result is sent as context'
              disabled={!explain.data}
              className='min-h-[60px] resize-none border-border bg-secondary pr-12 text-sm'
            />
            <Button
              size='icon'
              variant='default'
              onClick={submitFollowUp}
              disabled={!canSubmit}
              aria-label='Send follow-up'
              className='absolute top-1/2 right-2 -translate-y-1/2 transform'
            >
              <ArrowUp />
            </Button>
          </div>
        </SheetFooter>
      </SheetContent>
    </Sheet>
  );
}

function ObservedVsExpected({ anomaly }: { anomaly: MetricAnomaly }) {
  const deltaPct =
    Math.abs(anomaly.expected) < 1e-9
      ? null
      : ((anomaly.observed - anomaly.expected) / Math.abs(anomaly.expected)) * 100;
  return (
    <div className='grid grid-cols-3 gap-2 border border-border-subtle bg-field p-3'>
      <Stat label='Observed' value={formatNumber(anomaly.observed)} />
      <Stat label='Expected' value={formatNumber(anomaly.expected)} />
      <Stat
        label='Δ'
        value={
          deltaPct === null
            ? "—"
            : `${anomaly.observed >= anomaly.expected ? "+" : ""}${deltaPct.toFixed(1)}%`
        }
      />
    </div>
  );
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className='flex flex-col'>
      <span className='text-muted-foreground text-xs'>{label}</span>
      <span className='t-code font-medium text-sm'>{value}</span>
    </div>
  );
}

export function ExplainBody({ result }: { result: ExplainResult }) {
  const [warningsOpen, setWarningsOpen] = useState(false);
  const bodyRef = useRef<HTMLDivElement>(null);
  const [bodyHeight, setBodyHeight] = useState(0);

  useEffect(() => {
    const el = bodyRef.current;
    if (!el) return;
    const ro = new ResizeObserver(([entry]) => setBodyHeight(entry.contentRect.height));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Height of elements above the graph inside ExplainBody:
  // CoverageLine (~24px) + gap (12px) + Section label (~16px) + gap (6px) + TabsList (32px) + gap (8px) = ~98px
  const GRAPH_OVERHEAD = 100;
  const graphHeight = Math.max(180, bodyHeight - GRAPH_OVERHEAD);

  return (
    <div ref={bodyRef} className='flex min-h-0 flex-1 flex-col gap-3 overflow-auto'>
      <CoverageLine result={result} />
      {result.warnings && result.warnings.length > 0 && (
        <div>
          <button
            type='button'
            onClick={() => setWarningsOpen((o) => !o)}
            className='flex items-center gap-1 text-status-warning text-xs hover:underline'
          >
            {warningsOpen ? (
              <ChevronDown className='size-3' />
            ) : (
              <ChevronRight className='size-3' />
            )}
            {result.warnings.length} warning{result.warnings.length !== 1 ? "s" : ""}
          </button>
          {warningsOpen && (
            <ul className='mt-1 flex flex-col gap-1'>
              {result.warnings.map((w, i) => (
                <li
                  key={i}
                  className='border border-status-warning/40 bg-status-warning/10 p-2 text-status-warning text-xs'
                >
                  {warningMessage(w)}
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
      {result.nodes.length === 0 ? (
        <p className='text-muted-foreground text-sm'>
          No decomposition found — the move didn't concentrate in any single component or dimension
          above the significance threshold.
        </p>
      ) : (
        <Section title='Decomposition' className='min-h-0 flex-1'>
          {/* Graph view first — clicks/highlights the same data; the List
              tab is the fallback for screen readers and copy/paste. */}
          <Tabs defaultValue='graph' className='flex min-h-0 flex-1 flex-col gap-2'>
            <TabsList className='w-fit'>
              <TabsTrigger value='graph'>Graph</TabsTrigger>
              <TabsTrigger value='list'>List</TabsTrigger>
            </TabsList>
            <TabsContent
              value='graph'
              className='relative overflow-hidden border border-border bg-muted/20'
            >
              <ExplainGraph result={result} height={graphHeight} />
            </TabsContent>
            <TabsContent value='list'>
              <ul className='flex flex-col gap-1'>
                {result.nodes.map((n, i) => (
                  <ExplainNodeRow
                    key={`${n.measure}-${i}`}
                    node={n}
                    depth={0}
                    parentDelta={result.target_delta}
                  />
                ))}
              </ul>
            </TabsContent>
          </Tabs>
        </Section>
      )}
      <DriverSections drivers={result.driver_attribution} />
    </div>
  );
}

/** Driver attribution, split by whether each driver's move actually pushes the
 *  target the way it moved.
 *
 *  Keeping the two in one list made an offsetting driver read as a cause: a
 *  `direction: negative` driver that *fell* during a drop pushed the target
 *  *up*, so it dampened the anomaly rather than explaining it. The backend
 *  classifies (it holds the target delta); we only group and label. */
function DriverSections({ drivers }: { drivers?: DriverAttribution[] }) {
  if (!drivers || drivers.length === 0) return null;
  // Grouping is total by construction — see `groupDrivers`. Section order below
  // puts mechanical drivers after the sign split but ahead of the unresolved
  // fallback; within a group we keep the order airlayer sent.
  const { contributing, counteracting, mechanical, unresolved, anyStale } = groupDrivers(drivers);
  return (
    <>
      {contributing.length > 0 && (
        <Section title='Drivers explaining the move'>
          <ul className='flex flex-col gap-1'>
            {contributing.map((d) => (
              <DriverRow key={d.driver_measure} driver={d} />
            ))}
          </ul>
        </Section>
      )}
      {counteracting.length > 0 && (
        <Section title='Drivers offsetting the move'>
          <p className='text-muted-foreground text-xs'>
            These moved <em>against</em> the anomaly — they dampened it rather than causing it.
          </p>
          <ul className='flex flex-col gap-1'>
            {counteracting.map((d) => (
              <DriverRow key={d.driver_measure} driver={d} />
            ))}
          </ul>
        </Section>
      )}
      {mechanical.length > 0 && (
        <Section title='Drivers that moved mechanically'>
          <p className='text-muted-foreground text-xs'>
            These track another measure rather than moving on their own, so they say nothing about
            why the target moved. The rate is the part that carries a decision.
          </p>
          <ul className='flex flex-col gap-1'>
            {mechanical.map((d) => (
              <DriverRow key={d.driver_measure} driver={d} />
            ))}
          </ul>
        </Section>
      )}
      {unresolved.length > 0 && (
        <Section title='Drivers with undetermined direction'>
          <p className='text-muted-foreground text-xs'>
            {anyStale ? (
              <>
                This explain was cached before drivers were classified — hit Refresh to reclassify.
              </>
            ) : (
              <>
                Declare <code className='t-code'>direction</code> (or a{" "}
                <code className='t-code'>coefficient</code>) on these driver edges to place them.
              </>
            )}
          </p>
          <ul className='flex flex-col gap-1'>
            {unresolved.map((d) => (
              <DriverRow key={d.driver_measure} driver={d} />
            ))}
          </ul>
        </Section>
      )}
    </>
  );
}

function Section({
  title,
  children,
  className
}: {
  title: string;
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <div className={`flex flex-col gap-1.5${className ? ` ${className}` : ""}`}>
      <p className='text-muted-foreground text-xs uppercase tracking-wide'>{title}</p>
      {children}
    </div>
  );
}

/** Period-over-period delta + coverage % beneath the observed/expected card. */
function CoverageLine({ result }: { result: ExplainResult }) {
  return (
    <p className='text-muted-foreground text-xs'>
      Period-over-period:{" "}
      <span className='t-code text-foreground'>
        {formatNumber(result.target_previous)} → {formatNumber(result.target_current)}
      </span>{" "}
      ({formatSigned(result.target_delta)})
    </p>
  );
}

function DriverRow({ driver }: { driver: DriverAttribution }) {
  const impact = driver.estimated_target_impact;
  return (
    <li className='border border-border bg-card p-2 text-sm'>
      <div className='flex items-center justify-between gap-2'>
        <span className='font-medium'>{driver.driver_measure}</span>
        {impact !== undefined && impact !== null ? (
          <span className='text-muted-foreground text-xs tabular-nums'>
            est. impact {formatSigned(impact)}
          </span>
        ) : (
          // No coefficient on the edge, so there is no magnitude to quote —
          // say so rather than leaving the row looking like it has one.
          <span className='text-muted-foreground text-xs'>qualitative</span>
        )}
      </div>
      <p className='text-muted-foreground text-xs'>
        Δ {formatSigned(driver.driver_delta)} ({formatNumber(driver.driver_previous)} →{" "}
        {formatNumber(driver.driver_current)})
        {driver.direction &&
          driver.direction !== "unknown" &&
          ` · ${driver.direction} relationship`}
        {driver.coefficient !== undefined &&
          driver.coefficient !== null &&
          ` · coef ${driver.coefficient}`}
        {driver.form && ` · ${driver.form}`}
      </p>
      {driver.passthrough && <PassthroughLine split={driver.passthrough} />}
      {/* The description explains the *relationship*, not this period's move —
          it reads as a causal claim next to a delta that may run the other way,
          so label it. */}
      {driver.description && (
        <p className='mt-1 text-muted-foreground text-xs italic'>
          Relationship: {driver.description}
        </p>
      )}
    </li>
  );
}

/** Break a driver's move into the part its base forced and the part its own
 *  ratio contributed — the second number is the only one with a decision behind
 *  it, and it routinely points the opposite way to the raw delta. */
function PassthroughLine({ split }: { split: PassthroughSplit }) {
  const base = shortMeasureName(split.base_measure);
  return (
    <p className='mt-1 text-muted-foreground text-xs'>
      Tracks {base}:{" "}
      <span className='t-code'>
        {formatPercent(split.ratio_previous)} → {formatPercent(split.ratio_current)}
      </span>{" "}
      · {base}-driven {formatSigned(split.base_driven_delta)} · rate-driven{" "}
      {formatSigned(split.ratio_driven_delta)}
    </p>
  );
}

// ── derivations ────────────────────────────────────────────────────────────

/** Build (current, previous) period tuples for the explain call.
 *  current = the anomaly bucket; previous = one season back. */
function deriveExplainPeriods(a: MetricAnomaly): DerivedPeriods {
  const current = a.period_start.slice(0, 10);
  const baseline = sameCyclePrior(a);
  return { current: [current, current], previous: [baseline, baseline] };
}

function sameCyclePrior(a: MetricAnomaly): string {
  const start = new Date(a.period_start);
  const out = new Date(start);
  switch (a.granularity) {
    case "day":
    case "week":
      out.setUTCDate(out.getUTCDate() - 7);
      break;
    case "month":
      out.setUTCMonth(out.getUTCMonth() - 1);
      break;
    default:
      out.setUTCDate(out.getUTCDate() - 7);
  }
  return out.toISOString().slice(0, 10);
}
