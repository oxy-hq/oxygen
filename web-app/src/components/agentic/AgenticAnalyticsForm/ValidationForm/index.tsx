import { RotateCcw, SlidersHorizontal } from "lucide-react";
import { useId } from "react";
import { useFormContext } from "react-hook-form";
import { Button } from "@/components/ui/shadcn/button";
import { CardTitle } from "@/components/ui/shadcn/card";
import type { AgenticFormData } from "../index";
import { StageGroup } from "./components/StageGroup";
import { VALIDATION_STAGES } from "./constants";
import { defaultValidation } from "./utils";

export const ValidationForm: React.FC = () => {
  const { setValue, watch } = useFormContext<AgenticFormData>();
  const titleId = useId();
  // A `validation:` section replaces the built-in rule set rather than tuning
  // it, so the stages (whose field arrays would create the section just by
  // mounting) only render once the file has one.
  const customized = watch("validation") != null;

  return (
    <section aria-labelledby={titleId} className='space-y-4'>
      <div className='flex items-center justify-between gap-2'>
        <CardTitle id={titleId}>Validation</CardTitle>
        {customized ? (
          <Button
            type='button'
            variant='outline'
            size='sm'
            onClick={() => setValue("validation", undefined, { shouldDirty: true })}
          >
            <RotateCcw />
            Use Defaults
          </Button>
        ) : (
          <Button
            type='button'
            variant='outline'
            size='sm'
            onClick={() => setValue("validation", defaultValidation(), { shouldDirty: true })}
          >
            <SlidersHorizontal />
            Customize Rules
          </Button>
        )}
      </div>

      {customized ? (
        <>
          <p className='text-muted-foreground text-sm'>
            Only the rules listed here run. A stage with no rules is not checked, and a rule missing
            from this list does not run.
          </p>
          <div className='space-y-6 rounded-lg border p-4'>
            {VALIDATION_STAGES.map(({ stage, label, description }) => (
              <StageGroup key={stage} stage={stage} label={label} description={description} />
            ))}
          </div>
        </>
      ) : (
        <p className='text-muted-foreground text-sm'>
          Every built-in rule runs with its default parameters. Customize to switch a rule off or
          tune its thresholds.
        </p>
      )}
    </section>
  );
};
