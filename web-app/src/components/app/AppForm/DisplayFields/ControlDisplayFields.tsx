import { Plus, Trash2 } from "lucide-react";
import type React from "react";
import { useRef } from "react";
import type { FieldPath } from "react-hook-form";
import { Controller, useFormContext } from "react-hook-form";
import DateValueInput from "@/components/SemanticQueryPanel/DateValueInput";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { Switch } from "@/components/ui/shadcn/switch";
import type { AppFormData } from "../index";
import { TaskRefSelect } from "./components";

interface ControlDisplayFieldsProps {
  index: number;
}

const CONTROL_TYPES = [
  { value: "select", label: "Select (Dropdown)" },
  { value: "toggle", label: "Toggle" },
  { value: "date", label: "Date Picker" }
];

// ─── Options list editor ───────────────────────────────────────────────────────

interface OptionsEditorProps {
  options: string[];
  onChange: (options: string[]) => void;
}

const OptionsEditor: React.FC<OptionsEditorProps> = ({ options, onChange }) => {
  // Stable IDs per option row so React doesn't use array index as key.
  const idsRef = useRef<string[]>(options.map(() => crypto.randomUUID()));

  // Reconcile when options grow externally (e.g. form reset with more items).
  while (idsRef.current.length < options.length) {
    idsRef.current.push(crypto.randomUUID());
  }

  const update = (i: number, value: string) => {
    const next = [...options];
    next[i] = value;
    onChange(next);
  };

  const remove = (i: number) => {
    idsRef.current.splice(i, 1);
    onChange(options.filter((_, idx) => idx !== i));
  };

  const add = () => {
    idsRef.current.push(crypto.randomUUID());
    onChange([...options, ""]);
  };

  return (
    <div className='space-y-2'>
      {options.map((opt, i) => (
        <div key={idsRef.current[i]} className='flex items-center gap-2'>
          <Input
            value={opt}
            onChange={(e) => update(i, e.target.value)}
            placeholder={`Option ${i + 1}`}
            className='flex-1'
          />
          <Button type='button' variant='ghost' size='sm' onClick={() => remove(i)}>
            <Trash2 className='h-4 w-4' />
          </Button>
        </div>
      ))}
      <Button type='button' variant='outline' size='sm' onClick={add} className='w-full'>
        <Plus className='mr-1 h-3 w-3' />
        Add Option
      </Button>
    </div>
  );
};

// ─── Main component ────────────────────────────────────────────────────────────

// Helper to cast template-literal form paths that TypeScript can't statically
// resolve through the DisplayFormData index signature.
const fp = (path: string) => path as FieldPath<AppFormData>;

export const ControlDisplayFields: React.FC<ControlDisplayFieldsProps> = ({ index }) => {
  const { register, control, watch, setValue } = useFormContext<AppFormData>();
  const controlType = watch(`display.${index}.control_type`) as string | undefined;
  // Derive options mode directly from the watched source value — no local state
  // needed, so it can never desync from the form (e.g. on external reset).
  const source = watch(`display.${index}.source`) as string | undefined;
  const optionsMode: "static" | "dynamic" = source ? "dynamic" : "static";

  const switchToStatic = () => setValue(fp(`display.${index}.source`), undefined);
  const switchToDynamic = () => setValue(fp(`display.${index}.options`), undefined);

  return (
    <div className='space-y-4'>
      {/* Name */}
      <div className='space-y-2'>
        <Label htmlFor={`display.${index}.name`}>Name *</Label>
        <Input
          id={`display.${index}.name`}
          placeholder='e.g. region'
          {...register(`display.${index}.name`, { required: "Name is required" })}
        />
        <p className='text-muted-foreground text-sm'>
          Referenced in tasks as{" "}
          <code className='rounded bg-muted px-1 py-0.5 font-mono text-xs'>
            {"{{ controls.<name> }}"}
          </code>
        </p>
      </div>

      {/* Control type */}
      <div className='space-y-2'>
        <Label>Control Type *</Label>
        <Controller
          name={fp(`display.${index}.control_type`)}
          control={control}
          rules={{ required: "Control type is required" }}
          render={({ field }) => (
            <Select
              onValueChange={(value) => {
                field.onChange(value);
                setValue(fp(`display.${index}.options`), undefined);
                setValue(fp(`display.${index}.source`), undefined);
                setValue(fp(`display.${index}.default`), undefined);
              }}
              value={field.value as string | undefined}
            >
              <SelectTrigger>
                <SelectValue placeholder='Select control type' />
              </SelectTrigger>
              <SelectContent>
                {CONTROL_TYPES.map((t) => (
                  <SelectItem className='cursor-pointer' key={t.value} value={t.value}>
                    {t.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
        />
      </div>

      {/* Label */}
      <div className='space-y-2'>
        <Label htmlFor={`display.${index}.label`}>Label</Label>
        <Input
          id={`display.${index}.label`}
          placeholder='Display label shown above the control'
          {...register(`display.${index}.label`)}
        />
      </div>

      {/* Select-specific */}
      {controlType === "select" && (
        <>
          {/* Options source toggle */}
          <div className='space-y-3'>
            <div className='flex items-center justify-between'>
              <Label>Options</Label>
              <div className='flex rounded-md border p-0.5 text-sm'>
                <button
                  type='button'
                  onClick={switchToStatic}
                  className={`rounded px-3 py-1 transition-colors ${
                    optionsMode === "static"
                      ? "bg-background font-medium shadow-sm"
                      : "text-muted-foreground hover:text-foreground"
                  }`}
                >
                  Static list
                </button>
                <button
                  type='button'
                  onClick={switchToDynamic}
                  className={`rounded px-3 py-1 transition-colors ${
                    optionsMode === "dynamic"
                      ? "bg-background font-medium shadow-sm"
                      : "text-muted-foreground hover:text-foreground"
                  }`}
                >
                  From task
                </button>
              </div>
            </div>

            {optionsMode === "static" ? (
              <div className='space-y-2'>
                <Controller
                  control={control}
                  name={fp(`display.${index}.options`)}
                  render={({ field }) => {
                    const options = Array.isArray(field.value) ? (field.value as string[]) : [];
                    return <OptionsEditor options={options} onChange={field.onChange} />;
                  }}
                />
                <p className='text-muted-foreground text-sm'>
                  Supports Jinja templates, e.g.{" "}
                  <code className='rounded bg-muted px-1 py-0.5 font-mono text-xs'>
                    {"{{ now(fmt='%Y') }}"}
                  </code>
                </p>
              </div>
            ) : (
              <div className='space-y-2'>
                <Controller
                  control={control}
                  name={fp(`display.${index}.source`)}
                  render={({ field }) => (
                    <TaskRefSelect
                      value={field.value as string | undefined}
                      onChange={field.onChange}
                      placeholder='Select task...'
                    />
                  )}
                />
                <p className='text-muted-foreground text-sm'>
                  The first column of the selected task's output populates the dropdown.
                </p>
              </div>
            )}
          </div>

          {/* Default value */}
          <div className='space-y-2'>
            <Label htmlFor={`display.${index}.default`}>Default Value</Label>
            {/* Controlled rather than registered: a registered input writes ""
                for a control that has no default, and `default: ""` renders as
                '' where a missing default renders as none. Cleared means none,
                so "" reaches the YAML only when the file already said so. */}
            <Controller
              name={fp(`display.${index}.default`)}
              control={control}
              render={({ field }) => (
                <Input
                  id={`display.${index}.default`}
                  placeholder='e.g. All'
                  value={
                    typeof field.value === "string" || typeof field.value === "number"
                      ? String(field.value)
                      : ""
                  }
                  onChange={(e) =>
                    field.onChange(e.target.value === "" ? undefined : e.target.value)
                  }
                  onBlur={field.onBlur}
                />
              )}
            />
          </div>
        </>
      )}

      {/* Toggle-specific */}
      {controlType === "toggle" && (
        <div className='flex items-center gap-3'>
          <Label htmlFor={`display.${index}.default`}>Default State</Label>
          <Controller
            name={fp(`display.${index}.default`)}
            control={control}
            render={({ field }) => (
              <Switch
                id={`display.${index}.default`}
                checked={!!field.value}
                onCheckedChange={field.onChange}
              />
            )}
          />
        </div>
      )}

      {/* Date-specific */}
      {controlType === "date" && (
        <div className='space-y-2'>
          <Label>Default Date</Label>
          <Controller
            name={fp(`display.${index}.default`)}
            control={control}
            render={({ field }) => (
              <DateValueInput
                value={field.value as string | undefined}
                onChange={(v) => field.onChange(v || undefined)}
                placeholder='Pick a default date'
              />
            )}
          />
          <p className='text-muted-foreground text-sm'>
            Supports relative dates like "today" or "1 week ago".
          </p>
        </div>
      )}
    </div>
  );
};
