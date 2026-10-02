import { Plus, X } from "lucide-react";
import type React from "react";
import { Controller, useFieldArray, useWatch } from "react-hook-form";
import DateRangeSelector from "@/components/SemanticQueryPanel/DateRangeSelector";
import DateValueInput from "@/components/SemanticQueryPanel/DateValueInput";
import { Button } from "@/components/ui/shadcn/button";
import { Combobox } from "@/components/ui/shadcn/combobox";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { toText } from "@/libs/utils/string";
import type { SemanticQueryFilter } from "@/services/api/semantic";
import { FILTER_OPERATORS } from "./constants";
import type { FiltersFieldProps } from "./types";
import { getItemsWithUnknownValue } from "./utils";

const COMPARISON_OPS = ["eq", "neq", "gt", "gte", "lt", "lte"];
const ARRAY_OPS = ["in", "not_in"];
const DATE_RANGE_OPS = ["in_date_range", "not_in_date_range"];

function getInputType(dataType?: string): string {
  switch (dataType) {
    case "number":
      return "number";
    default:
      return "text";
  }
}

function getPlaceholder(dataType?: string): string {
  switch (dataType) {
    case "number":
      return "Enter a number";
    case "date":
    case "datetime":
      return "Select a date";
    case "boolean":
      return "true or false";
    default:
      return "Enter a value";
  }
}

function getArrayPlaceholder(dataType?: string): string {
  switch (dataType) {
    case "number":
      return "1, 2, 3";
    case "date":
    case "datetime":
      return "2024-01-01, 2024-12-31";
    default:
      return "Value1, Value2, Value3";
  }
}

interface FilterRowProps {
  taskPath: string;
  filterIndex: number;
  control: FiltersFieldProps["control"];
  topicValue: string | undefined;
  fieldsLoading: boolean;
  allFieldItems: FiltersFieldProps["allFieldItems"];
  onRemove: () => void;
}

const FilterRow: React.FC<FilterRowProps> = ({
  taskPath,
  filterIndex,
  control,
  topicValue,
  fieldsLoading,
  allFieldItems,
  onRemove
}) => {
  const filterPath = `${taskPath}.filters.${filterIndex}` as const;

  // Watch the current filter to react to field/op changes
  // @ts-expect-error - dynamic field path
  const currentFilter = useWatch({ control, name: filterPath }) as SemanticQueryFilter | undefined;

  const currentFieldValue = currentFilter?.field;
  const currentOp = currentFilter?.op;

  const selectedFieldItem = allFieldItems.find((f) => f.value === currentFieldValue);
  const isTimeDimension =
    selectedFieldItem?.dataType === "date" || selectedFieldItem?.dataType === "datetime";

  const availableOperators = FILTER_OPERATORS.filter((op) => {
    if (DATE_RANGE_OPS.includes(op.value)) {
      return isTimeDimension;
    }
    return true;
  });

  return (
    <div className='flex items-center gap-2'>
      <div className='flex-1'>
        <Controller
          control={control}
          // @ts-expect-error - dynamic field path
          name={`${filterPath}.field`}
          render={({ field: controllerField }) => {
            const value = controllerField.value as string;
            const items = getItemsWithUnknownValue(allFieldItems, value);
            return (
              <Combobox
                items={items}
                value={value}
                onValueChange={controllerField.onChange}
                placeholder='Select field...'
                searchPlaceholder='Search fields...'
                disabled={!topicValue || fieldsLoading}
              />
            );
          }}
        />
      </div>
      <Controller
        control={control}
        // @ts-expect-error - dynamic field path
        name={filterPath}
        render={({ field }) => {
          const filter = field.value as SemanticQueryFilter;
          const handleOperatorChange = (newOp: string) => {
            if (ARRAY_OPS.includes(newOp)) {
              field.onChange({
                field: filter.field,
                op: newOp,
                values: "values" in filter ? filter.values : []
              });
            } else if (DATE_RANGE_OPS.includes(newOp)) {
              const existing = filter as Extract<
                SemanticQueryFilter,
                { op: "in_date_range" | "not_in_date_range" }
              >;
              field.onChange({
                field: filter.field,
                op: newOp,
                relative: existing.relative,
                from: existing.from,
                to: existing.to
              });
            } else {
              field.onChange({
                field: filter.field,
                op: newOp,
                value: "value" in filter ? filter.value : ""
              });
            }
          };

          return (
            <Select value={filter?.op ?? "eq"} onValueChange={handleOperatorChange}>
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {availableOperators.map((op) => (
                  <SelectItem className='cursor-pointer' key={op.value} value={op.value}>
                    {op.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          );
        }}
      />

      {currentOp && COMPARISON_OPS.includes(currentOp) && (
        <div className='flex-1'>
          <Controller
            control={control}
            // @ts-expect-error - dynamic field path
            name={`${filterPath}.value`}
            render={({ field }) =>
              isTimeDimension ? (
                <DateValueInput
                  value={field.value != null ? toText(field.value) : undefined}
                  onChange={(val) => field.onChange(val ?? "")}
                  placeholder={getPlaceholder(selectedFieldItem?.dataType)}
                />
              ) : (
                <Input
                  type={getInputType(selectedFieldItem?.dataType)}
                  placeholder={getPlaceholder(selectedFieldItem?.dataType)}
                  value={toText(field.value)}
                  onChange={(e) => {
                    const val = e.target.value;
                    field.onChange(
                      selectedFieldItem?.dataType === "number" && val ? Number(val) : val
                    );
                  }}
                />
              )
            }
          />
        </div>
      )}

      {currentOp && ARRAY_OPS.includes(currentOp) && (
        <div className='flex-1'>
          <Controller
            control={control}
            // @ts-expect-error - dynamic field path
            name={`${filterPath}.values`}
            render={({ field }) => {
              const values = (field.value as unknown[]) ?? [];
              return (
                <Input
                  type='text'
                  placeholder={getArrayPlaceholder(selectedFieldItem?.dataType)}
                  defaultValue={values.join(", ")}
                  onBlur={(e) => {
                    const parsed = e.target.value
                      .split(",")
                      .map((v) => v.trim())
                      .filter(Boolean);
                    field.onChange(
                      selectedFieldItem?.dataType === "number"
                        ? parsed.map((v) => Number(v))
                        : parsed
                    );
                  }}
                />
              );
            }}
          />
        </div>
      )}

      {currentOp && DATE_RANGE_OPS.includes(currentOp) && (
        <div className='flex-1'>
          <Controller
            control={control}
            // @ts-expect-error - dynamic field path
            name={filterPath}
            render={({ field }) => {
              const filter = field.value as Extract<
                SemanticQueryFilter,
                { op: "in_date_range" | "not_in_date_range" }
              >;
              return (
                <DateRangeSelector
                  from={filter?.from}
                  to={filter?.to}
                  onChange={(updates) =>
                    field.onChange({
                      ...filter,
                      ...updates
                    })
                  }
                />
              );
            }}
          />
        </div>
      )}

      <Button type='button' onClick={onRemove} variant='ghost' size='sm'>
        <X className='h-4 w-4' />
      </Button>
    </div>
  );
};

export const FiltersField: React.FC<FiltersFieldProps> = ({
  taskPath,
  control,
  topicValue,
  fieldsLoading,
  allFieldItems
}) => {
  const {
    fields: filterFields,
    append: appendFilter,
    remove: removeFilter
  } = useFieldArray({
    control,
    // @ts-expect-error - dynamic field path
    name: `${taskPath}.filters`
  });

  return (
    <div className='space-y-2'>
      <div className='flex items-center justify-between'>
        <Label>Filters</Label>
        <Button
          type='button'
          onClick={() => appendFilter({ field: "", op: "eq", value: "" } as never)}
          variant='outline'
          size='sm'
          disabled={!topicValue}
        >
          <Plus className='mr-1 h-4 w-4' />
          Add Filter
        </Button>
      </div>
      {filterFields.length > 0 && (
        <div className='space-y-2'>
          {filterFields.map((field, filterIndex) => (
            <FilterRow
              key={field.id}
              taskPath={taskPath}
              filterIndex={filterIndex}
              control={control}
              topicValue={topicValue}
              fieldsLoading={fieldsLoading}
              allFieldItems={allFieldItems}
              onRemove={() => removeFilter(filterIndex)}
            />
          ))}
        </div>
      )}
    </div>
  );
};
