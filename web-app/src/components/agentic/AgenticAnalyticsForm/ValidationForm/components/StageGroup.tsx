import { Plus } from "lucide-react";
import { useFieldArray, useFormContext } from "react-hook-form";
import { Button } from "@/components/ui/shadcn/button";
import type { AgenticFormData } from "../../index";
import type { ValidationStage } from "../constants";
import { RuleItem } from "./RuleItem";

interface StageGroupProps {
  stage: ValidationStage;
  label: string;
  description: string;
}

export const StageGroup: React.FC<StageGroupProps> = ({ stage, label, description }) => {
  const { control } = useFormContext<AgenticFormData>();
  const { fields, append, remove } = useFieldArray({
    control,
    name: `validation.rules.${stage}`
  });

  return (
    <div className='space-y-3'>
      <div className='flex items-center justify-between gap-2'>
        <div>
          <p className='font-medium text-sm'>{label}</p>
          <p className='text-muted-foreground text-xs'>{description}</p>
        </div>
        <Button
          type='button'
          variant='outline'
          size='sm'
          aria-label={`Add rule ${label.toLowerCase()}`}
          onClick={() => append({ enabled: true })}
        >
          <Plus />
          Add Rule
        </Button>
      </div>
      {fields.length === 0 && (
        <p className='py-2 text-center text-muted-foreground text-sm'>
          No rules. Nothing is checked at this stage.
        </p>
      )}
      <div className='space-y-2'>
        {fields.map((field, index) => (
          <RuleItem key={field.id} stage={stage} index={index} onRemove={() => remove(index)} />
        ))}
      </div>
    </div>
  );
};
