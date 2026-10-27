import { Input } from "@/components/ui/shadcn/input";
import { periodWord, switchInputType } from "@/pages/ide/Experiments/grain";
import type { FieldsProps } from "./fieldsProps";
import PriceField from "./PriceField";
import { startNote } from "./startNote";

/**
 * The inputs of a staggered design. Sizes and blocks count units, or groups once a
 * dimension is set. Matched pairs replace both: the server pairs every unit by
 * baseline and treats one of each pair on the first switch date.
 */
export default function WavesFields({
  index,
  register,
  messageFor,
  matchPairs,
  grain
}: FieldsProps & { matchPairs: boolean }) {
  const id = `experiment-price-${index}`;
  const path = (field: "waveSizes" | "firstSwitchOn" | "spacingDays" | "blocks" | "assignBy") =>
    register(`candidates.${index}.${field}`);
  return (
    <div className='grid gap-3 sm:grid-cols-2'>
      <label className='flex items-start gap-2 text-sm sm:col-span-2'>
        <input
          type='checkbox'
          className='mt-0.5'
          data-testid={`${id}-match-pairs`}
          {...register(`candidates.${index}.matchPairs`)}
        />
        <span>
          Match in pairs
          <span className='block text-muted-foreground text-xs'>
            For a few units: pairs them by baseline volume and treats one of each pair on the first
            switch date.
          </span>
        </span>
      </label>
      {!matchPairs && (
        <PriceField
          id={`${id}-wave-sizes`}
          label='Wave sizes'
          hint='Units (or groups) per wave, comma-separated, for example 3, 3, 2. Each wave needs at least 2.'
          error={messageFor("waveSizes")}
        >
          <Input
            id={`${id}-wave-sizes`}
            data-testid={`${id}-wave-sizes`}
            placeholder='3, 3'
            aria-invalid={Boolean(messageFor("waveSizes"))}
            {...path("waveSizes")}
          />
        </PriceField>
      )}
      <PriceField
        id={`${id}-first-switch`}
        label='First switch date'
        hint={`A calendar date in the experiment's own time zone.${startNote(grain)}`}
        error={messageFor("firstSwitchOn")}
      >
        <Input
          id={`${id}-first-switch`}
          data-testid={`${id}-first-switch`}
          type={switchInputType(grain)}
          aria-invalid={Boolean(messageFor("firstSwitchOn"))}
          {...path("firstSwitchOn")}
        />
      </PriceField>
      {!matchPairs && (
        <PriceField
          id={`${id}-spacing`}
          label={`Spacing between waves (${periodWord(grain, { plural: true })})`}
          error={messageFor("spacingDays")}
        >
          <Input
            id={`${id}-spacing`}
            data-testid={`${id}-spacing`}
            inputMode='numeric'
            aria-invalid={Boolean(messageFor("spacingDays"))}
            {...path("spacingDays")}
          />
        </PriceField>
      )}
      {!matchPairs && (
        <PriceField
          id={`${id}-blocks`}
          label='Blocks (optional)'
          hint='2 or more ranks units by baseline volume and gives every wave its share of each block.'
          error={messageFor("blocks")}
        >
          <Input
            id={`${id}-blocks`}
            data-testid={`${id}-blocks`}
            inputMode='numeric'
            aria-invalid={Boolean(messageFor("blocks"))}
            {...path("blocks")}
          />
        </PriceField>
      )}
      <PriceField
        id={`${id}-assign-by`}
        label='Assign by a dimension (optional)'
        hint="A dimension of the experiment's entity, such as a category: its values, not single units, are randomised. The target must be a sum or a count."
        error={messageFor("assignBy")}
      >
        <Input
          id={`${id}-assign-by`}
          data-testid={`${id}-assign-by`}
          placeholder='category'
          aria-invalid={Boolean(messageFor("assignBy"))}
          {...path("assignBy")}
        />
      </PriceField>
    </div>
  );
}
