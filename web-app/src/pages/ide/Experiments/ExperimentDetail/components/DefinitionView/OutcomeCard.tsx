import { grainOf, periodWord } from "@/pages/ide/Experiments/grain";
import { StatGrid, StatTile } from "../../../components/glance/StatTile";
import { formatGroupDay, formatUnitDay } from "../../../format";
import {
  confidenceText,
  directionText,
  filterText,
  measureLabel,
  percent,
  plural,
  significanceText
} from "./describe";
import { DASH, orDash, type SafeDefinition } from "./safe";
import { Chip, LabelledChip } from "./shared";

const tile = (id: string) => `definition-stat-${id}`;

/** The design's key facts as tiles, then the few outcome extras as chips. */
export default function OutcomeCard({ def }: { def: SafeDefinition }) {
  const o = def.outcome ?? {};
  const d = def.design ?? {};
  const grain = grainOf(d.grain);
  const many = periodWord(grain, { plural: true });
  const perPeriod = def.assign_by ? formatGroupDay : formatUnitDay;
  const mde = (v: number) => perPeriod(v, grain);
  const waves = d.waves ?? [];
  const periods = d.switchback?.periods ?? [];
  const guardrails = def.guardrails ?? [];
  const scheduleValue =
    waves.length > 0
      ? plural(waves.length, "wave")
      : periods.length > 0
        ? plural(periods.length, "period")
        : DASH;
  const priced = scheduleValue !== DASH;
  return (
    <div data-testid='definition-outcome' className='space-y-2'>
      <StatGrid className='sm:grid-cols-4'>
        <StatTile
          id='target'
          testId={tile("target")}
          label='Target measure'
          value={o.target ? measureLabel(o.target) : DASH}
          sub={
            o.target ? (
              <>
                <Chip>{o.target}</Chip>
                {o.target_filter?.length ? (
                  <span data-testid='definition-target-filter'> {filterText(o.target_filter)}</span>
                ) : null}
              </>
            ) : undefined
          }
          hint='The measure the experiment is trying to move.'
        />
        <StatTile
          id='direction'
          testId={tile("direction")}
          label='Expected direction'
          value={o.direction ? (o.direction === "increase" ? "Increase" : "Decrease") : DASH}
          sub={o.direction ? directionText(o.direction) : undefined}
          hint='The direction promised before the run. The test is judged against this.'
        />
        <StatTile
          id='confidence'
          testId={tile("confidence")}
          label='Confidence'
          value={orDash(o.alpha, confidenceText)}
          sub={orDash(o.alpha, significanceText)}
          hint='How sure the result must be before it counts: one minus the allowed false-alarm rate (alpha).'
        />
        <StatTile
          id='power'
          testId={tile("power")}
          label='Power'
          value={orDash(o.power, percent)}
          sub='chance to detect the promised effect'
          hint='If the promised effect is real, the share of runs that would find it.'
        />
        <StatTile
          id='mde'
          testId={tile("mde")}
          label='Promised min. effect'
          value={o.mde_at_proposal == null ? DASH : mde(o.mde_at_proposal)}
          sub={o.mde_at_proposal === null ? "Not priced yet" : undefined}
          hint='The smallest effect the design was priced to detect (minimum detectable effect).'
        />
        <StatTile
          id='schedule'
          testId={tile("schedule")}
          label={d.switchback ? "Periods" : "Waves"}
          value={scheduleValue}
          sub={priced ? undefined : "Not priced yet"}
          hint='How many switch dates the schedule has. Details are under Schedule.'
        />
        <StatTile
          id='window'
          testId={tile("window")}
          label={`Pre / post ${many}`}
          value={
            d.pre_days == null || d.post_days == null ? DASH : `${d.pre_days} / ${d.post_days}`
          }
          sub={`${many} before / after each switch`}
          hint='How much history before and after each switch is compared.'
        />
        <StatTile
          id='guardrails'
          testId={tile("guardrails")}
          label='Guardrails'
          value={guardrails.length}
          sub={def.substitution_check ? "+ substitution check" : undefined}
          hint='Checks that abort the run if something else breaks.'
        />
      </StatGrid>
      <div className='flex flex-wrap items-center gap-1.5 text-xs'>
        {o.driver ? (
          <>
            <LabelledChip label='Driver'>{o.driver}</LabelledChip>
            {o.driver_filter?.length ? (
              <span className='text-muted-foreground'>{filterText(o.driver_filter)}</span>
            ) : null}
          </>
        ) : (
          <span className='text-muted-foreground'>No driver set</span>
        )}
        {o.prior_coefficient && (
          <span className='text-muted-foreground'>
            World Model prior: slope {o.prior_coefficient.value} ({o.prior_coefficient.source})
          </span>
        )}
        {(o.family ?? 1) > 1 && (
          <span className='text-muted-foreground'>
            {plural(o.family ?? 1, "outcome")} registered together, so the test is corrected for it
          </span>
        )}
      </div>
    </div>
  );
}
