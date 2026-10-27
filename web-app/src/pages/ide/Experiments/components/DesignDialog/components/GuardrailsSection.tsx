import { Plus, X } from "lucide-react";
import { useFieldArray } from "react-hook-form";
import { Button } from "@/components/ui/shadcn/button";
import { newGuardrail } from "../designForm";
import ConditionField from "./ConditionField";
import { FormSection, Grid, type SectionProps, SelectField, TextField } from "./fields";
import { MeasureField } from "./pickers";

/** Conditions that abort the run, one row each. */
export default function GuardrailsSection(p: SectionProps) {
  const { fields, append, remove } = useFieldArray({ control: p.form.control, name: "guardrails" });
  const rows = p.form.watch("guardrails");
  return (
    <FormSection
      id='guardrails'
      title='Guardrails'
      description='Each guardrail aborts the run when it trips.'
    >
      {fields.length === 0 && <p className='text-muted-foreground text-sm'>No guardrails.</p>}
      {fields.map((field, i) => (
        <div
          key={field.id}
          className='flex flex-col gap-3 rounded-md border p-3'
          data-testid={`experiment-design-guardrail-${i}`}
        >
          <div className='flex items-center justify-between'>
            <span className='font-medium text-sm'>Guardrail {i + 1}: aborts the run</span>
            <Button
              type='button'
              variant='ghost'
              size='sm'
              onClick={() => remove(i)}
              data-testid={`experiment-design-guardrail-${i}-remove`}
            >
              <X className='h-4 w-4' />
              Remove
            </Button>
          </div>
          <SelectField
            {...p}
            name={`guardrails.${i}.kind`}
            label='Trips when'
            choices={[
              { value: "anomaly", label: "A measure looks anomalous" },
              { value: "trigger", label: "A condition holds" }
            ]}
          />
          {rows[i]?.kind === "trigger" ? (
            <ConditionField
              {...p}
              name={`guardrails.${i}.when`}
              label='Condition'
              hint='Divide by a second measure to compare a ratio, such as ad spend over sales.'
              ratio
            />
          ) : (
            <Grid>
              <MeasureField
                {...p}
                name={`guardrails.${i}.measure`}
                label='Measure to watch'
                placeholder='orders.refund_rate'
              />
              <TextField
                {...p}
                name={`guardrails.${i}.significance`}
                label='Significance'
                type='number'
                min={0}
                max={1}
                step='any'
              />
            </Grid>
          )}
        </div>
      ))}
      <div>
        <Button
          type='button'
          variant='outline'
          size='sm'
          onClick={() => append(newGuardrail())}
          data-testid='experiment-design-add-guardrail'
        >
          <Plus className='h-4 w-4' />
          Add a guardrail
        </Button>
      </div>
    </FormSection>
  );
}
