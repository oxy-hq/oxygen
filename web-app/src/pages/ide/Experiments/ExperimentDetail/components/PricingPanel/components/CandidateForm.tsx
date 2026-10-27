import { Loader2, Plus } from "lucide-react";
import type { BaseSyntheticEvent } from "react";
import { type UseFormReturn, useFieldArray } from "react-hook-form";
import { Button } from "@/components/ui/shadcn/button";
import { FieldError } from "@/components/ui/shadcn/field";
import type { Grain } from "@/pages/ide/Experiments/grain";
import {
  type CandidateField,
  type CandidateShape,
  expandedCount,
  MAX_CANDIDATES,
  newCandidate,
  type PriceFormValues
} from "../priceForm";
import CandidateRow from "./CandidateRow";

/** The error shapes the resolver produces, read loosely: RHF's `Merge<…>` types do not index cleanly. */
type RowErrors = Partial<Record<CandidateField, { message?: string }>>;
type ListErrors = (RowErrors | undefined)[] & { root?: { message?: string } };

/**
 * The candidate list. The count shown, and checked, is the EXPANDED one: a row
 * priced at both alphas is two candidates. The set-level error (none, or too
 * many) comes from the resolver as the field array's `root` error.
 */
export default function CandidateForm({
  form,
  shape,
  onSubmit,
  disabled,
  denied = false,
  busy,
  assignments,
  grain
}: {
  form: UseFormReturn<PriceFormValues>;
  /** The shape a new row starts in: the draft's own. */
  shape: CandidateShape;
  onSubmit: (event?: BaseSyntheticEvent) => Promise<void>;
  disabled: boolean;
  /** The caller lacks the editor role the price route needs: say so beside the disabled button. */
  denied?: boolean;
  busy: boolean;
  assignments: number;
  grain: Grain;
}) {
  const { control, register, watch, formState } = form;
  const { fields, append, remove } = useFieldArray({ control, name: "candidates" });
  const rows = watch("candidates");
  const count = expandedCount(rows);
  const errors = formState.errors.candidates as unknown as ListErrors | undefined;

  return (
    <form
      onSubmit={onSubmit}
      noValidate
      className='flex flex-col gap-4'
      data-testid='experiment-price-form'
    >
      {fields.map((field, index) => (
        <CandidateRow
          key={field.id}
          index={index}
          shape={rows[index]?.shape ?? shape}
          matchPairs={rows[index]?.matchPairs ?? false}
          register={register}
          grain={grain}
          messageFor={(f: CandidateField) => errors?.[index]?.[f]?.message}
          onRemove={fields.length > 1 ? () => remove(index) : null}
        />
      ))}
      <div className='flex flex-wrap items-center gap-3'>
        <Button
          type='button'
          variant='outline'
          size='sm'
          disabled={fields.length >= MAX_CANDIDATES}
          onClick={() => append(newCandidate(shape))}
          data-testid='experiment-price-add-candidate'
        >
          <Plus className='h-4 w-4' />
          Add a candidate
        </Button>
        <p
          className={
            count > MAX_CANDIDATES ? "text-destructive text-sm" : "text-muted-foreground text-sm"
          }
          data-testid='experiment-price-count'
        >
          {count} of {MAX_CANDIDATES} candidates
        </p>
      </div>
      {errors?.root?.message && (
        <FieldError data-testid='experiment-price-candidates-error'>
          {errors.root.message}
        </FieldError>
      )}
      <div className='flex flex-wrap items-center gap-3'>
        <Button type='submit' disabled={disabled || busy} data-testid='experiment-price-submit'>
          {busy && <Loader2 className='h-4 w-4 animate-spin' />}
          {count > 1 ? `Price ${count} candidates` : "Price this design"}
        </Button>
        {denied && (
          <p className='text-muted-foreground text-xs' data-testid='experiment-price-denied'>
            Only a workspace editor or above can price a design.
          </p>
        )}
        <p className='text-muted-foreground text-xs' data-testid='experiment-proposal-count'>
          {assignments} {assignments === 1 ? "assignment" : "assignments"} proposed so far.
        </p>
      </div>
    </form>
  );
}
