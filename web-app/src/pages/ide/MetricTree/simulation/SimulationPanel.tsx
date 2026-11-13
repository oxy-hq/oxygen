import { useMemo, useState } from "react";
import { toast } from "sonner";
import { Echarts } from "@/components/Echarts";
import {
  useSimulationRun,
  useSimulationRuns,
  useSimulations,
  useStartSimulationRun
} from "@/hooks/api/useSimulation";
import useTheme from "@/stores/useTheme";
import { POLICIES } from "@/types/simulation";
import { SectionHeader } from "../../components/semanticGraph";
import { convergenceOptions, profitRaceOptions } from "./charts";
import { EdgeLabel } from "./EdgeLabel";
import { type Arm, PeriodStepper, RACE, RunPicker, WorldsBar } from "./RunControls";
import { TruthBadge } from "./TruthBadge";

/**
 * The demo surface: pick a declared world, run it, and watch the estimate walk
 * toward a truth the model was never told.
 *
 * Laid out as one stack of labelled sections — the same `SectionHeader` +
 * dense-row language the Explore and Scenario panels use, because all three
 * share this slot and only differ by which mode is selected.
 */

/** Shorter than the 400px default: this is a side panel, and two charts plus
 *  the controls above them have to be reachable without a long scroll. */
const CHART_HEIGHT = "220px";

export default function SimulationPanel() {
  const { data: worlds = [], isLoading: worldsLoading } = useSimulations();
  const { data: runs = [] } = useSimulationRuns();
  const startRun = useStartSimulationRun();
  const [selectedRun, setSelectedRun] = useState<string | undefined>();
  const [selectedWorld, setSelectedWorld] = useState<string | undefined>();
  const [period, setPeriod] = useState<number | null>(null);
  // The product is the default arm: a run nobody parameterised should be a run
  // of the thing we ship.
  const [arm, setArm] = useState<Arm>("machine");

  // Defaults to the first declared world until someone picks a different one.
  const worldName = selectedWorld ?? worlds[0]?.name;

  const runId = selectedRun ?? runs[0]?.run_id;
  const { data: detail } = useSimulationRun(runId);

  // Null period means "follow the run" — the stepper tracks the head until
  // someone scrubs, so a live run advances on its own and a finished one opens
  // on its last period.
  const maxPeriod = detail?.periods.length ?? 0;
  const shownPeriod = period ?? maxPeriod;

  const fitsAtPeriod = useMemo(
    () => detail?.fits.filter((f) => f.period === shownPeriod) ?? [],
    [detail, shownPeriod]
  );
  // Through the store, not `document.documentElement.classList` — reading the
  // DOM at render would not re-run when the theme changes, leaving the charts
  // in the previous palette until something else re-rendered them.
  const { theme } = useTheme();
  const isDark = theme === "dark";

  const edges = useMemo(() => {
    const seen = new Set<string>();
    return (detail?.fits ?? []).filter((f) => {
      if (seen.has(f.edge)) return false;
      seen.add(f.edge);
      return true;
    });
  }, [detail]);

  return (
    <div className='flex flex-col gap-4 p-4' data-testid='metric-tree-simulation-panel'>
      <WorldsBar
        worlds={worlds}
        isLoading={worldsLoading}
        isPending={startRun.isPending}
        world={worldName}
        onWorldChange={setSelectedWorld}
        arm={arm}
        onArmChange={setArm}
        onRun={() => {
          if (!worldName) return;
          startRun.mutate(
            { name: worldName, policies: arm === RACE ? POLICIES : [arm] },
            {
              onSuccess: (queued) => {
                // The first arm queued, which is `hold` in a race — the null
                // every other curve is read against.
                setSelectedRun(queued.runs[0]?.run_id);
                setPeriod(null);
                // Some arms queued and are executing; the rest never were. A
                // race missing an arm is worth saying out loud, because the
                // chart below will simply draw fewer curves.
                if (queued.partial_failure) {
                  toast.warning(queued.partial_failure);
                }
              }
            }
          );
        }}
      />

      <RunPicker
        runs={runs}
        runId={runId}
        onSelect={(next) => {
          setSelectedRun(next);
          setPeriod(null);
        }}
      />

      {runs.length === 0 && (
        <p className='text-[11px] text-muted-foreground leading-relaxed'>
          No runs yet. Run a world to watch its estimate walk toward a truth the model was never
          told.
        </p>
      )}

      {detail && (
        <>
          {detail.run.status === "failed" && (
            <p className='rounded-md border border-destructive/40 bg-destructive/10 p-2 text-[11px] text-destructive leading-relaxed'>
              {detail.run.error ?? "the run failed"}
            </p>
          )}

          <PeriodStepper
            period={period}
            shownPeriod={shownPeriod}
            maxPeriod={maxPeriod}
            planned={detail.run.periods_planned}
            onScrub={setPeriod}
            onFollow={() => setPeriod(null)}
          />

          <section className='flex flex-col gap-1.5'>
            <SectionHeader title='Fits' subtitle={`period ${shownPeriod}`} />
            {fitsAtPeriod.length === 0 ? (
              <p className='text-[11px] text-muted-foreground'>no fit at this period yet</p>
            ) : (
              // One per row. Two columns fit at the panel's widest and nowhere
              // else, and a grid that drops to one column at most widths is
              // just a stack that changes its mind.
              <div className='flex flex-col gap-1.5'>
                {fitsAtPeriod.map((f) => (
                  <TruthBadge key={f.edge} fit={f} />
                ))}
              </div>
            )}
          </section>

          {edges.map((edge) => (
            <section key={edge.edge} className='flex flex-col gap-1'>
              <SectionHeader title='Convergence' subtitle={`${edge.form} basis`} />
              <EdgeLabel edge={edge.edge} />
              <Echarts
                isLoading={false}
                height={CHART_HEIGHT}
                options={convergenceOptions(
                  detail.fits.filter((f) => f.edge === edge.edge),
                  isDark
                )}
              />
            </section>
          ))}

          <section className='flex flex-col gap-1'>
            <SectionHeader title='Profit' subtitle='cumulative' />
            <Echarts
              isLoading={false}
              height={CHART_HEIGHT}
              options={profitRaceOptions(
                [
                  {
                    label: `${detail.run.simulation_name} (${detail.run.policy})`,
                    periods: detail.periods
                  }
                ],
                isDark
              )}
            />
          </section>
        </>
      )}
    </div>
  );
}
