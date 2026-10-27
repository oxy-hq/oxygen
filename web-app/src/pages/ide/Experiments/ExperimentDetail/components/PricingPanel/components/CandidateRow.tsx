import { X } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import type { AlphaChoice, CandidateShape } from "../priceForm";
import type { FieldsProps } from "./fieldsProps";
import SwitchbackFields from "./SwitchbackFields";
import WavesFields from "./WavesFields";

const SHAPES: { value: CandidateShape; label: string }[] = [
  { value: "waves", label: "Staggered waves" },
  { value: "switchback", label: "Switchback" }
];
const ALPHAS: { value: AlphaChoice; label: string }[] = [
  { value: "0.05", label: "0.05" },
  { value: "0.2", label: "0.2 (directional)" },
  { value: "both", label: "Both" }
];

/**
 * One candidate design. Plain radios rather than a select: they are visible,
 * keyboard-reachable, and a single click in the browser flow. The fields of the
 * shape not chosen are unmounted, not cleared — they keep what was typed.
 */
export default function CandidateRow(
  props: FieldsProps & {
    shape: CandidateShape;
    matchPairs: boolean;
    onRemove: (() => void) | null;
  }
) {
  const { index, register, shape, onRemove } = props;
  const id = `experiment-price-${index}`;
  return (
    <fieldset
      className='flex flex-col gap-3 rounded-lg border p-3'
      data-testid={`experiment-price-candidate-${index}`}
    >
      <legend className='px-1 font-medium text-sm'>Candidate {index + 1}</legend>
      <div className='flex flex-wrap items-center gap-x-6 gap-y-2 text-sm'>
        <div role='radiogroup' aria-label='Design' className='flex gap-4'>
          {SHAPES.map((s) => (
            <label key={s.value} className='flex items-center gap-1.5'>
              <input
                type='radio'
                value={s.value}
                data-testid={`${id}-shape-${s.value}`}
                {...register(`candidates.${index}.shape`)}
              />
              {s.label}
            </label>
          ))}
        </div>
        <div role='radiogroup' aria-label='Significance level (alpha)' className='flex gap-4'>
          <span className='text-muted-foreground'>α</span>
          {ALPHAS.map((a) => (
            <label key={a.value} className='flex items-center gap-1.5'>
              <input
                type='radio'
                value={a.value}
                data-testid={`${id}-alpha-${a.value}`}
                {...register(`candidates.${index}.alpha`)}
              />
              {a.label}
            </label>
          ))}
        </div>
        {onRemove && (
          <Button
            type='button'
            variant='ghost'
            size='sm'
            className='ml-auto'
            onClick={onRemove}
            data-testid={`${id}-remove`}
          >
            <X className='h-4 w-4' />
            Remove
          </Button>
        )}
      </div>
      {shape === "waves" ? (
        <WavesFields {...props} matchPairs={props.matchPairs} />
      ) : (
        <SwitchbackFields {...props} />
      )}
    </fieldset>
  );
}
