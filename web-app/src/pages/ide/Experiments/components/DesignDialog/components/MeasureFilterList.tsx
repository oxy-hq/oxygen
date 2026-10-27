import { Check, ChevronsUpDown, Plus, X } from "lucide-react";
import { useState } from "react";
import { useController, useFieldArray } from "react-hook-form";
import { Button } from "@/components/ui/shadcn/button";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList
} from "@/components/ui/shadcn/command";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/shadcn/popover";
import { useDimensionValues, useMeasureDimensions } from "@/hooks/api/useMeasureFilterOptions";
import { newFilter } from "../designForm";
import { DesignField, fieldId, type SectionProps, SelectField } from "./fields";
import { PickerField } from "./PickerField";

type FilterList = "targetFilter" | "driverFilter" | "siblingFilter";

const OPS = [
  { value: "equals", label: "is one of" },
  { value: "not_equals", label: "is not one of" }
] as const;

/** The values a filter keeps: picked from the dimension's distinct values, never typed. */
function ValuesField(
  p: SectionProps & { name: `${FilterList}.${number}.values`; dimension: string; measure: string }
) {
  const { field } = useController({ control: p.form.control, name: p.name });
  const picked: string[] = Array.isArray(field.value) ? field.value : [];
  const { data: values = [], isLoading, isError } = useDimensionValues(p.dimension, p.measure);
  const [open, setOpen] = useState(false);
  const id = fieldId(p.name);
  const toggle = (v: string) =>
    field.onChange(picked.includes(v) ? picked.filter((x) => x !== v) : [...picked, v]);
  const empty = !p.dimension
    ? "Choose a dimension first."
    : isLoading
      ? "Reading its values…"
      : isError
        ? "Its values could not be read."
        : "No value matches.";
  return (
    <DesignField id={id} label='Values' error={p.messageFor(p.name)}>
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <Button
            id={id}
            data-testid={id}
            type='button'
            variant='outline'
            className='w-full justify-between font-normal'
            disabled={!p.dimension}
          >
            <span className='truncate'>
              {picked.length > 0 ? picked.join(", ") : "Choose values"}
            </span>
            <ChevronsUpDown className='h-4 w-4 opacity-50' />
          </Button>
        </PopoverTrigger>
        <PopoverContent align='start' className='w-[var(--radix-popover-trigger-width)] p-0'>
          <Command>
            <CommandInput placeholder='Search values' />
            <CommandList>
              <CommandEmpty>{empty}</CommandEmpty>
              <CommandGroup>
                {values.map((v) => (
                  <CommandItem key={v} value={v} onSelect={() => toggle(v)}>
                    <Check className={picked.includes(v) ? "h-4 w-4" : "h-4 w-4 opacity-0"} />
                    {v}
                  </CommandItem>
                ))}
              </CommandGroup>
            </CommandList>
          </Command>
        </PopoverContent>
      </Popover>
    </DesignField>
  );
}

/**
 * Narrows one measure to some rows of its view, such as one product's sales.
 * The dimension comes from the measure's own view; the values from the data.
 */
export default function MeasureFilterList(
  p: SectionProps & { name: FilterList; measure: string; title: string }
) {
  const { fields, append, remove } = useFieldArray({ control: p.form.control, name: p.name });
  const rows = p.form.watch(p.name) ?? [];
  const { view, dimensions } = useMeasureDimensions(p.measure);
  const known = new Set(dimensions.map((d) => `${view}.${d.name}`));
  const groups = [
    {
      title: view,
      options: dimensions.map((d) => ({
        value: `${view}.${d.name}`,
        label: d.name,
        detail: d.description
      }))
    }
  ];
  const testId = fieldId(p.name);
  return (
    <div className='flex flex-col gap-2' data-testid={testId}>
      {fields.map((f, i) => {
        const dimension = rows[i]?.dimension ?? "";
        return (
          <div key={f.id} className='flex items-start gap-2' data-testid={`${testId}-${i}`}>
            <div className='grid flex-1 grid-cols-3 gap-2'>
              <PickerField
                {...p}
                name={`${p.name}.${i}.dimension`}
                label={i === 0 ? p.title : "and"}
                groups={groups}
                listLabel='Dimensions'
                match={
                  dimension && dimensions.length > 0 && !known.has(dimension)
                    ? { tone: "warning", text: `Not a dimension of ${view}` }
                    : null
                }
              />
              <SelectField
                {...p}
                name={`${p.name}.${i}.op`}
                label='Keeps rows where it'
                choices={[...OPS]}
              />
              <ValuesField
                {...p}
                name={`${p.name}.${i}.values`}
                dimension={dimension}
                measure={p.measure}
              />
            </div>
            <Button
              type='button'
              variant='ghost'
              size='sm'
              className='mt-6'
              aria-label={`Remove filter ${i + 1}`}
              onClick={() => remove(i)}
            >
              <X className='h-4 w-4' />
            </Button>
          </div>
        );
      })}
      <div>
        <Button
          type='button'
          variant='ghost'
          size='sm'
          disabled={!view}
          onClick={() => append(newFilter())}
          data-testid={`${testId}-add`}
        >
          <Plus className='h-4 w-4' />
          {fields.length === 0 ? "Narrow to some rows" : "Add another filter"}
        </Button>
      </div>
    </div>
  );
}
