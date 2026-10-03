import { Trash2 } from "lucide-react";
import { useId } from "react";
import { Controller, useFormContext } from "react-hook-form";
import { Button } from "@/components/ui/shadcn/button";
import { Checkbox } from "@/components/ui/shadcn/checkbox";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import type { AgenticFormData } from "../../index";
import { RULE_PARAM_FIELDS, RULES_BY_STAGE, type ValidationStage } from "../constants";

interface RuleItemProps {
  stage: ValidationStage;
  index: number;
  onRemove: () => void;
}

export const RuleItem: React.FC<RuleItemProps> = ({ stage, index, onRemove }) => {
  const { control, register, watch } = useFormContext<AgenticFormData>();
  const path = `validation.rules.${stage}.${index}` as const;
  const id = useId();
  const name = watch(`${path}.name`);

  const known = RULES_BY_STAGE[stage];
  const current = known.find((rule) => rule.value === name);
  // A name this form does not know (a newer backend, a typo) stays selectable,
  // so the select shows what the file says instead of a blank.
  const options = name && !current ? [...known, { value: name, label: name }] : known;
  const ruleLabel = current?.label ?? (name || "New rule");
  const params = (name && RULE_PARAM_FIELDS[name]) || [];

  return (
    <fieldset aria-label={ruleLabel} className='min-w-0 space-y-3 rounded-lg border p-3'>
      <div className='flex items-end gap-3'>
        <div className='flex-1 space-y-2'>
          <Label htmlFor={`${id}-name`}>
            Rule{" "}
            <span aria-hidden='true' className='text-destructive'>
              *
            </span>
          </Label>
          <Controller
            name={`${path}.name`}
            control={control}
            render={({ field }) => (
              <Select required onValueChange={field.onChange} value={field.value ?? ""}>
                <SelectTrigger id={`${id}-name`}>
                  <SelectValue placeholder='Select rule' />
                </SelectTrigger>
                <SelectContent>
                  {options.map((opt) => (
                    <SelectItem className='cursor-pointer' key={opt.value} value={opt.value}>
                      {opt.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            )}
          />
        </div>
        <div className='flex h-9 items-center gap-2'>
          <Controller
            name={`${path}.enabled`}
            control={control}
            render={({ field }) => (
              <Checkbox
                id={`${id}-enabled`}
                // The backend treats a missing `enabled` as true.
                checked={field.value ?? true}
                onCheckedChange={(checked) => field.onChange(checked === true)}
              />
            )}
          />
          <Label htmlFor={`${id}-enabled`}>Enabled</Label>
        </div>
        <Button
          type='button'
          variant='ghost'
          size='sm'
          aria-label={`Remove ${ruleLabel}`}
          onClick={onRemove}
        >
          <Trash2 className='h-4 w-4' />
        </Button>
      </div>

      {params.length > 0 && (
        <div className='grid grid-cols-2 gap-3'>
          {params.map((param) => (
            <div key={param.key} className='space-y-2'>
              <Label htmlFor={`${id}-${param.key}`}>{param.label}</Label>
              <Input
                id={`${id}-${param.key}`}
                type='number'
                step={param.step}
                min={param.min}
                max={param.max}
                placeholder={param.placeholder}
                {...register(`${path}.${param.key}`, { valueAsNumber: true })}
              />
            </div>
          ))}
        </div>
      )}
    </fieldset>
  );
};
