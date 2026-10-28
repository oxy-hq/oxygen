import { periods as countOf, GRAIN_CHOICES, grainOf } from "@/pages/ide/Experiments/grain";
import HowCalculated from "../ResultPane/components/HowCalculated";
import { periodsText, plural, windowsText } from "./describe";
import { orDash, type SafeDefinition } from "./safe";
import { Block, Chip, LabelledChip } from "./shared";

type Design = NonNullable<SafeDefinition["design"]>;
type Wave = { switch_on: string; units: string[] };

function Waves({ waves, noun }: { waves: Wave[]; noun: string }) {
  return (
    <table className='w-full text-left'>
      <thead className='text-muted-foreground'>
        <tr>
          <th className='py-1 pr-4 font-normal'>Wave</th>
          <th className='py-1 pr-4 font-normal'>Switches on</th>
          <th className='py-1 font-normal'>Covers</th>
        </tr>
      </thead>
      <tbody>
        {waves.map((w, i) => (
          <tr key={w.switch_on} className='border-t align-top'>
            <td className='py-1 pr-4'>{i + 1}</td>
            <td className='py-1 pr-4'>{w.switch_on}</td>
            <td className='py-1'>
              <details>
                <summary className='cursor-pointer'>{plural(w.units.length, noun)}</summary>
                <ul className='mt-1 max-h-40 overflow-auto font-mono'>
                  {w.units.map((u) => (
                    <li key={u}>{u}</li>
                  ))}
                </ul>
              </details>
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

export default function ScheduleCard({
  design,
  noun,
  past = false
}: {
  design: SafeDefinition["design"];
  noun: string;
  /** Read from past data: nothing was assigned, so there is no seed to show. */
  past?: boolean;
}) {
  const d: Design = design ?? {};
  const waves = (d.waves ?? []) as Wave[];
  const sb = d.switchback ?? null;
  const periods = sb?.periods ?? [];
  const priced = waves.length > 0 || periods.length > 0;
  const grain = grainOf(d.grain);
  const windows = windowsText(
    {
      pre_days: d.pre_days ?? null,
      post_days: d.post_days ?? null,
      anticipation_days: d.anticipation_days ?? 0,
      washout_days: d.washout_days ?? 0
    },
    grain
  );
  return (
    <Block id='schedule' title='Schedule'>
      {sb && sb.period_days !== undefined && (
        <p className='text-sm'>{periodsText({ period_days: sb.period_days, periods }, grain)}</p>
      )}
      {!priced && !sb && (
        <p className='text-muted-foreground text-sm'>Not priced yet — price a design below.</p>
      )}
      {d.carryover_days ? (
        <p className='text-sm' data-testid='definition-carryover'>
          Keeps reading {countOf(d.carryover_days, grain)} after the lever turns off, for the effect
          that lingers.
        </p>
      ) : null}
      <div className='flex flex-wrap items-center gap-1.5'>
        <LabelledChip label='Time zone'>{orDash(d.timezone)}</LabelledChip>
        <LabelledChip label='Calendar'>
          {d.time_dimension ?? "No date dimension named"}
        </LabelledChip>
        <LabelledChip label='Measured per'>
          {GRAIN_CHOICES.find((c) => c.value === grain)?.label}
        </LabelledChip>
        {!past && (
          <LabelledChip label='Seed'>{d.seed == null ? "No seed yet" : d.seed}</LabelledChip>
        )}
        {d.strata && (
          <LabelledChip label='Blocks'>
            {plural(d.strata.length, "block")} by baseline volume (
            {d.strata.map((s) => s.length).join(", ")} {noun}s)
          </LabelledChip>
        )}
      </div>
      <HowCalculated title='Windows and buffers'>
        <p>{windows}</p>
        <p>
          {past ? "" : d.seed == null ? "No seed yet. " : `Assignment seed ${d.seed}. `}
          Calendar: {d.time_dimension ? <Chip>{d.time_dimension}</Chip> : "No date dimension named"}
          .
        </p>
      </HowCalculated>
      {waves.length > 0 && (
        <HowCalculated title='Waves and units'>
          <Waves waves={waves} noun={noun} />
        </HowCalculated>
      )}
      {periods.length > 0 && (
        <HowCalculated title='Every period'>
          <ul className='max-h-40 overflow-auto'>
            {periods.map((p) => (
              <li key={p.from}>
                {p.from}: {p.state}
              </li>
            ))}
          </ul>
        </HowCalculated>
      )}
    </Block>
  );
}
