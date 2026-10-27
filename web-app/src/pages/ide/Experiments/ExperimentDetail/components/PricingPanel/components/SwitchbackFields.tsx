import { Input } from "@/components/ui/shadcn/input";
import { periodWord, switchInputType } from "@/pages/ide/Experiments/grain";
import type { FieldsProps } from "./fieldsProps";
import PriceField from "./PriceField";
import { startNote } from "./startNote";

/** The inputs of a switchback: the whole fleet alternates on and off in a seeded random order within each pair. */
export default function SwitchbackFields({ index, register, messageFor, grain }: FieldsProps) {
  const id = `experiment-price-${index}`;
  return (
    <div className='grid gap-3 sm:grid-cols-3'>
      <PriceField
        id={`${id}-first-period`}
        label='First period starts'
        hint={`A calendar date in the experiment's own time zone.${startNote(grain)}`}
        error={messageFor("firstPeriodOn")}
      >
        <Input
          id={`${id}-first-period`}
          data-testid={`${id}-first-period`}
          type={switchInputType(grain)}
          aria-invalid={Boolean(messageFor("firstPeriodOn"))}
          {...register(`candidates.${index}.firstPeriodOn`)}
        />
      </PriceField>
      <PriceField
        id={`${id}-period-days`}
        label={`Period length (${periodWord(grain, { plural: true })})`}
        hint={
          grain === "day"
            ? "Whole weeks (7, 14) hold every weekday equally often."
            : grain === "hour"
              ? "Any number of hours, such as 4 or 24."
              : undefined
        }
        error={messageFor("periodDays")}
      >
        <Input
          id={`${id}-period-days`}
          data-testid={`${id}-period-days`}
          inputMode='numeric'
          aria-invalid={Boolean(messageFor("periodDays"))}
          {...register(`candidates.${index}.periodDays`)}
        />
      </PriceField>
      <PriceField
        id={`${id}-pairs`}
        label='Pairs'
        hint='One on and one off period each. At α 0.05 the test needs at least 6 pairs to reach the level at all, at 0.2 at least 4; fewer is refused.'
        error={messageFor("pairs")}
      >
        <Input
          id={`${id}-pairs`}
          data-testid={`${id}-pairs`}
          inputMode='numeric'
          aria-invalid={Boolean(messageFor("pairs"))}
          {...register(`candidates.${index}.pairs`)}
        />
      </PriceField>
    </div>
  );
}
