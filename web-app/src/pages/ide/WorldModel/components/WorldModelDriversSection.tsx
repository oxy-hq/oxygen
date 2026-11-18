import { ChevronDown, ChevronRight } from "lucide-react";
import { useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { TooltipWrapper } from "@/components/ui/shadcn/utils/with-tooltip";
import { useSensitivity } from "@/hooks/api/useMetricTree";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import { EMPTY_SCENARIO, encodeScenario } from "@/pages/ide/MetricTree/scenario/scenarioUrl";
import useCurrentOrg from "@/stores/useCurrentOrg";
import type { SensitivityDriver } from "@/types/metricTree";
import {
  InfoTip,
  MagnitudeBar,
  SectionHeader,
  SectionSpinner
} from "../../components/semanticGraph";
import { byLeverage, formatBeta, shortMeasureName } from "./measureTarget";
import { WorldModelWhatIf } from "./WorldModelWhatIf";

/**
 * Deep-link the current measure into the Metric Tree's Scenario mode, pinned
 * as a lever. The Metric Tree lives on the Semantic Layer IDE tab (selected
 * via `?view=metric-tree`, not its own route path), and that page's own
 * `useSearchParams()` reads `mode` + the `encodeScenario` params off the same
 * query string — so all three key sets share one URL.
 */
function useScenarioHref(nodeId: string): string {
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  return useMemo(() => {
    const params = encodeScenario({
      ...EMPTY_SCENARIO,
      // "+0", not "": `decodeScenario` round-trips the empty string faithfully
      // and `resolveLever` maps it to `not_a_number`, so "Simulate this →"
      // opened the canvas with a destructive-red error under an input nobody
      // had touched. A signed zero delta is the one mode that needs no
      // baseline, and resolves to the deliberate no-op a fresh lever wants.
      // `pinLever` in `MetricTree/index.tsx` seeds the same value for the same
      // reason — these are the two entry points into a pinned scenario.
      levers: [{ nodeId, raw: "+0" }]
    });
    params.set("view", "metric-tree");
    params.set("mode", "scenario");
    const semanticRoot = ROUTES.ORG(orgSlug).WORKSPACE().IDE.SEMANTIC.ROOT;
    return `${semanticRoot}?${params.toString()}`;
  }, [nodeId, orgSlug]);
}

const DIRECTION_GLYPH: Record<string, string> = {
  positive: "↑",
  negative: "↓",
  unknown: "?"
};

interface WorldModelDriversSectionProps {
  /** Metric-tree node id (`view.measure`) of the selected measure. */
  nodeId: string;
  /** Navigate to a driver's own node (it is a measure elsewhere in the graph). */
  onSelectDriver: (driverNodeId: string) => void;
  /** Whether a given driver resolves to a selectable graph node. */
  canSelectDriver: (driverNodeId: string) => boolean;
}

/**
 * What actually moves this measure, and by how much.
 *
 * This is the "how do we improve X" surface: the metric tree's driver edges
 * ranked by elasticity (β — the cumulative chain-rule coefficient along the
 * path), plus a what-if that propagates a hypothetical move on one driver back
 * up to the target. Each driver is itself a node in the graph, so its row
 * navigates there.
 *
 * Both `sensitivity` and `predict` are pure metric-tree walks server-side — no
 * warehouse query — so this section loads eagerly and the what-if re-runs on
 * every change without cost.
 */
export function WorldModelDriversSection({
  nodeId,
  onSelectDriver,
  canSelectDriver
}: WorldModelDriversSectionProps) {
  const [open, setOpen] = useState(true);
  const sensitivity = useSensitivity(open ? nodeId : undefined);
  const scenarioHref = useScenarioHref(nodeId);

  const drivers = useMemo(
    () => [...(sensitivity.data?.drivers ?? [])].sort(byLeverage),
    [sensitivity.data]
  );
  // Normalize β strength bars to the strongest driver in view.
  const maxBeta = useMemo(
    () => Math.max(0, ...drivers.map((d) => Math.abs(d.effective_coefficient ?? 0))),
    [drivers]
  );

  return (
    <section className='flex flex-col gap-1.5 border-border border-t pt-3'>
      <button
        type='button'
        onClick={() => setOpen((v) => !v)}
        className='flex w-full items-center gap-1 text-left'
        aria-expanded={open}
        data-testid={`wm-drivers-toggle-${nodeId}`}
      >
        {open ? (
          <ChevronDown size={12} className='shrink-0 text-muted-foreground' />
        ) : (
          <ChevronRight size={12} className='shrink-0 text-muted-foreground' />
        )}
        <span className='min-w-0 flex-1'>
          <SectionHeader title='Drivers' subtitle='what moves this measure' />
        </span>
      </button>

      {open && (
        <div className='flex flex-col gap-2 pt-1'>
          {sensitivity.isPending && <SectionSpinner />}

          {sensitivity.error && (
            <p className='font-mono text-[10px] text-destructive leading-relaxed'>
              {sensitivity.error instanceof Error
                ? sensitivity.error.message
                : "Failed to load drivers."}
            </p>
          )}

          {sensitivity.data && drivers.length === 0 && (
            <p className='font-mono text-[10px] text-muted-foreground leading-relaxed'>
              No drivers declared for this measure — the metric tree has no component or driver
              edges feeding it, so there is no lever to pull.
            </p>
          )}

          {drivers.length > 0 && (
            <>
              <div className='flex items-center gap-1 font-mono text-[9.5px] text-muted-foreground'>
                β · modelled leverage
                <InfoTip
                  content={`β is elasticity along the path — the modelled change in ${shortMeasureName(
                    nodeId
                  )} per unit change in the driver. "—" means the edge carries no quantitative coefficient. Click a driver to open its own node.`}
                />
              </div>
              <div className='flex flex-col gap-1'>
                {drivers.map((d) => (
                  <DriverRow
                    key={d.measure}
                    driver={d}
                    target={nodeId}
                    maxBeta={maxBeta}
                    selectable={canSelectDriver(d.measure)}
                    onSelect={() => onSelectDriver(d.measure)}
                  />
                ))}
              </div>
              <WorldModelWhatIf drivers={drivers} target={nodeId} />
            </>
          )}

          <TooltipWrapper tooltip='Pin this measure as a lever on the metric tree and see everything it moves.'>
            <Link
              to={scenarioHref}
              className='self-start font-mono text-[9.5px] text-info hover:underline'
              data-testid={`wm-simulate-this-${nodeId}`}
            >
              Simulate this →
            </Link>
          </TooltipWrapper>
        </div>
      )}
    </section>
  );
}

function DriverRow({
  driver,
  target,
  maxBeta,
  selectable,
  onSelect
}: {
  driver: SensitivityDriver;
  target: string;
  maxBeta: number;
  selectable: boolean;
  onSelect: () => void;
}) {
  // path is [driver, …intermediates, target]; anything longer than 2 reaches the
  // target through another measure, which changes how you'd act on it.
  const transitive = driver.path.length > 2;
  const beta = driver.effective_coefficient;
  const fraction = beta != null && maxBeta > 0 ? Math.abs(beta) / maxBeta : 0;

  const head = (
    <span className='flex min-w-0 items-center justify-between gap-2'>
      <span className='flex min-w-0 items-center gap-1.5'>
        <span className='shrink-0 text-muted-foreground'>
          {DIRECTION_GLYPH[driver.direction] ?? "?"}
        </span>
        <span className='truncate text-foreground'>{shortMeasureName(driver.measure)}</span>
      </span>
      <span className='flex shrink-0 items-center gap-1.5'>
        <span className={cn(beta == null ? "text-muted-foreground" : "text-info")}>
          β {formatBeta(beta)}
        </span>
        <span className='text-muted-foreground'>{driver.strength}</span>
      </span>
    </span>
  );

  const inner = (
    <>
      {head}
      {beta != null && <MagnitudeBar fraction={fraction} className='mt-1' />}
    </>
  );

  const boxClass =
    "flex flex-col border border-border bg-background/40 px-2 py-1.5 font-mono text-xs";

  return (
    <div className='flex flex-col gap-0.5' data-testid={`wm-driver-${driver.measure}`}>
      {selectable ? (
        <button
          type='button'
          onClick={onSelect}
          className={cn(boxClass, "text-left transition-colors hover:border-info/60")}
          title={`Open ${shortMeasureName(driver.measure)}`}
        >
          {inner}
        </button>
      ) : (
        <div className={boxClass}>{inner}</div>
      )}
      {transitive && (
        <div className='truncate pl-2 font-mono text-[9.5px] text-muted-foreground'>
          via {driver.path.slice(1, -1).map(shortMeasureName).join(" → ")} →{" "}
          {shortMeasureName(target)}
        </div>
      )}
      {driver.lag != null && driver.lag > 0 && (
        <div className='pl-2 font-mono text-[9.5px] text-muted-foreground'>
          lags {driver.lag} period{driver.lag === 1 ? "" : "s"}
        </div>
      )}
    </div>
  );
}
