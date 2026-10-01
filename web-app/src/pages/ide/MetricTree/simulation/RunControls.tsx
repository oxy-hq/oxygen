import { Pencil, Play, Plus } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { Slider } from "@/components/ui/shadcn/slider";
import {
  POLICIES,
  type Policy,
  type SimulationRun,
  type SimulationSummary
} from "@/types/simulation";
import { SectionHeader } from "../../components/semanticGraph";

/**
 * The three controls above the charts: pick a world and policy then run them,
 * pick which run is on screen, pick which period of it.
 *
 * Selecting a world and running it are two separate steps (a dropdown, then a
 * dedicated Run button) rather than one click-to-run row per world — a row
 * that both selected and ran read as "expand this" on first glance, and
 * separating them also gives "which world is armed" a resting state instead
 * of only existing mid-click.
 *
 * All three are full-width rows rather than an inline toolbar. The panel is
 * resizable from ~340px, and world names and run labels are long enough that a
 * wrapping toolbar re-flowed into a different shape at every width.
 */

interface WorldsBarProps {
  worlds: SimulationSummary[];
  isLoading: boolean;
  isPending: boolean;
  /** Which declared world the Run button acts on. */
  world: string | undefined;
  onWorldChange: (name: string) => void;
  /** Which arm (or every arm) the next run is. */
  arm: Arm;
  onArmChange: (arm: Arm) => void;
  onRun: () => void;
  onNewWorld: () => void;
  /** Disabled when no world is selected — there is nothing to edit yet. */
  onEditWorld: () => void;
}

/** `race` queues every arm of one world at once — the profit race, which is only
 *  attributable because all five see the same seed and the same shocks. That it
 *  is one click is the point: when each arm was its own `.simulation.yml`, the
 *  files could drift and the comparison quietly stopped being one. */
export const RACE = "race";

/** One arm, or all of them. */
export type Arm = Policy | typeof RACE;

const ARMS: Arm[] = [RACE, ...POLICIES];

export function WorldsBar({
  worlds,
  isLoading,
  isPending,
  world,
  onWorldChange,
  arm,
  onArmChange,
  onRun,
  onNewWorld,
  onEditWorld
}: WorldsBarProps) {
  return (
    <section className='flex flex-col gap-1.5'>
      <div className='flex items-baseline justify-between gap-2'>
        <SectionHeader
          title='Worlds'
          subtitle={isLoading ? "loading…" : `${worlds.length} declared`}
        />
        <button
          type='button'
          onClick={onNewWorld}
          aria-label='New world'
          data-testid='simulation-new-world-button'
          className='flex shrink-0 items-center gap-1 text-[9.5px] text-muted-foreground uppercase tracking-wider hover:text-foreground'
        >
          <Plus className='size-3' />
          New
        </button>
      </div>
      {!isLoading && worlds.length === 0 ? (
        <p className='text-[11px] text-muted-foreground leading-relaxed'>
          No <span className='font-mono'>.simulation.yml</span> on this branch — start one with{" "}
          <span className='font-mono'>New</span> above, rather than hand-writing the file.
        </p>
      ) : (
        <>
          {/* Selecting is picking which declared world the Run button below
              acts on — it does not itself start anything. */}
          <div className='flex items-center gap-1.5'>
            <Select value={world ?? ""} onValueChange={onWorldChange}>
              <SelectTrigger
                size='sm'
                aria-label='World to run'
                className='h-7 w-full font-mono text-xs'
                data-testid='simulation-world-picker'
              >
                <SelectValue placeholder='select a world' />
              </SelectTrigger>
              <SelectContent>
                {worlds.map((w) => (
                  <SelectItem key={w.name} value={w.name} className='font-mono text-xs'>
                    {w.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Button
              type='button'
              variant='ghost'
              size='sm'
              aria-label={world ? `Edit ${world}` : "Edit world"}
              data-testid='simulation-edit-world-button'
              className='h-7 shrink-0 px-2'
              disabled={!world}
              onClick={onEditWorld}
            >
              <Pencil className='size-3' />
            </Button>
          </div>
          {/* The arm is picked here rather than declared in the world, because a
              world is what happens and a policy is what someone does about it. */}
          <Select value={arm} onValueChange={(next) => onArmChange(next as Arm)}>
            <SelectTrigger
              size='sm'
              aria-label='Policy to run'
              className='h-7 w-full font-mono text-xs'
              data-testid='simulation-policy-picker'
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {ARMS.map((a) => (
                <SelectItem key={a} value={a} className='font-mono text-xs'>
                  {a === RACE ? "race · every arm" : a}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Button
            type='button'
            variant='default'
            size='sm'
            aria-label={world ? `Run ${world}` : "Run"}
            data-testid='simulation-run-button'
            className='h-7 w-full gap-1.5 font-mono text-xs'
            disabled={isPending || !world}
            onClick={onRun}
          >
            <Play className='size-3 shrink-0' />
            Run
          </Button>
        </>
      )}
    </section>
  );
}

/** `marketing_lift · machine · done (30/30)` — the world, the policy it ran,
 *  and how far it got, in the order you'd ask for them.
 *
 *  A replicate index is shown only past the first draw: every run would
 *  otherwise carry a `#0` that means nothing on the worlds that declare one
 *  replicate, which is most of them. */
function runLabel(run: SimulationRun): string {
  const draw = run.replicate > 0 ? ` #${run.replicate}` : "";
  return `${run.simulation_name} · ${run.policy}${draw} · ${run.status} (${run.periods_done}/${run.periods_planned})`;
}

interface RunPickerProps {
  runs: SimulationRun[];
  runId: string | undefined;
  onSelect: (runId: string) => void;
}

export function RunPicker({ runs, runId, onSelect }: RunPickerProps) {
  if (runs.length === 0) return null;

  return (
    <section className='flex flex-col gap-1.5'>
      <SectionHeader title='Run' subtitle={`${runs.length} total`} />
      <Select value={runId ?? ""} onValueChange={onSelect}>
        <SelectTrigger
          size='sm'
          aria-label='Run to show'
          className='h-7 w-full font-mono text-xs'
          data-testid='simulation-run-picker'
        >
          <SelectValue placeholder='no run selected' />
        </SelectTrigger>
        <SelectContent>
          {runs.map((r) => (
            <SelectItem key={r.run_id} value={r.run_id} className='font-mono text-xs'>
              {runLabel(r)}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </section>
  );
}

interface PeriodStepperProps {
  /** `null` means "follow the run" — the stepper tracks the head. */
  period: number | null;
  shownPeriod: number;
  maxPeriod: number;
  planned: number;
  onScrub: (period: number) => void;
  onFollow: () => void;
}

export function PeriodStepper({
  period,
  shownPeriod,
  maxPeriod,
  planned,
  onScrub,
  onFollow
}: PeriodStepperProps) {
  return (
    <section className='flex flex-col gap-1.5'>
      <SectionHeader title='Period' subtitle={`${shownPeriod} / ${planned}`} />
      <div className='flex items-center gap-2'>
        <Slider
          className='min-w-0 flex-1'
          aria-label='Period'
          min={1}
          max={Math.max(maxPeriod, 1)}
          step={1}
          value={[Math.max(shownPeriod, 1)]}
          onValueChange={([next]) => onScrub(next)}
        />
        {/* Always mounted, disabled while following: mounting it on first scrub
            resized the slider under the cursor mid-drag. */}
        <Button
          type='button'
          variant='ghost'
          size='sm'
          className='h-6 shrink-0 px-1.5 text-[10px] text-muted-foreground'
          disabled={period === null}
          onClick={onFollow}
        >
          follow
        </Button>
      </div>
    </section>
  );
}
