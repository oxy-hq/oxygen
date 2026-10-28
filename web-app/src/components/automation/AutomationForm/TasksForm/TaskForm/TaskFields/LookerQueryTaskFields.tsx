import { Plus, X } from "lucide-react";
import type React from "react";
import { Controller, useFieldArray, useFormContext } from "react-hook-form";
import { Button } from "@/components/ui/shadcn/button";
import { FieldError } from "@/components/ui/shadcn/field";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import useLookerIntegrations from "@/hooks/api/integrations/useLookerIntegrations";
import type { AutomationFormData } from "../..";

interface LookerQueryTaskFieldsProps {
  index: number;
  basePath?: string;
}

export const LookerQueryTaskFields: React.FC<LookerQueryTaskFieldsProps> = ({
  index,
  basePath = "tasks"
}) => {
  const {
    register,
    control,
    watch,
    setValue,
    formState: { errors }
  } = useFormContext<AutomationFormData>();

  const taskPath = `${basePath}.${index}`;
  // @ts-expect-error - Dynamic path for nested tasks
  const taskErrors = errors[basePath]?.[index];

  // @ts-expect-error - dynamic field path
  const selectedIntegration = watch(`${taskPath}.integration`) as string | undefined;
  // @ts-expect-error - dynamic field path
  const selectedModel = watch(`${taskPath}.model`) as string | undefined;

  const { data: lookerIntegrations = [] } = useLookerIntegrations();

  const currentIntegration = lookerIntegrations.find((i) => i.name === selectedIntegration);

  const availableModels = currentIntegration
    ? [...new Set(currentIntegration.explores.map((e) => e.model))]
    : [];

  const availableExplores = currentIntegration
    ? currentIntegration.explores.filter((e) => e.model === selectedModel)
    : [];

  const {
    fields: fieldEntries,
    append: appendField,
    remove: removeField
  } = useFieldArray({
    control,
    // @ts-expect-error - dynamic field path
    name: `${taskPath}.fields`
  });

  const {
    fields: filterEntries,
    append: appendFilter,
    remove: removeFilter
  } = useFieldArray({
    control,
    // @ts-expect-error - dynamic field path
    name: `${taskPath}.filters`
  });

  const {
    fields: sortEntries,
    append: appendSort,
    remove: removeSort
  } = useFieldArray({
    control,
    // @ts-expect-error - dynamic field path
    name: `${taskPath}.sorts`
  });

  return (
    <div className='space-y-4'>
      <div className='space-y-2'>
        <Label htmlFor={`${taskPath}.integration`}>Integration</Label>
        <Controller
          control={control}
          // @ts-expect-error - dynamic field path
          name={`${taskPath}.integration`}
          rules={{ required: "Integration is required" }}
          render={({ field }) => (
            <Select
              value={(field.value as string) || ""}
              onValueChange={(value) => {
                field.onChange(value);
                // @ts-expect-error - dynamic field path
                setValue(`${taskPath}.model`, "");
                // @ts-expect-error - dynamic field path
                setValue(`${taskPath}.explore`, "");
              }}
            >
              <SelectTrigger className='w-full'>
                <SelectValue placeholder='Select integration' />
              </SelectTrigger>
              <SelectContent className='min-w-(--radix-select-trigger-width)'>
                {lookerIntegrations.map((integration) => (
                  <SelectItem key={integration.name} value={integration.name}>
                    <span className='block truncate' title={integration.name}>
                      {integration.name}
                    </span>
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
        />
        {taskErrors?.integration && <FieldError>{taskErrors.integration.message}</FieldError>}
      </div>

      <div className='grid grid-cols-2 gap-4'>
        <div className='space-y-2'>
          <Label htmlFor={`${taskPath}.model`}>Model</Label>
          <Controller
            control={control}
            // @ts-expect-error - dynamic field path
            name={`${taskPath}.model`}
            rules={{ required: "Model is required" }}
            render={({ field }) => (
              <Select
                value={(field.value as string) || ""}
                disabled={!selectedIntegration}
                onValueChange={(value) => {
                  field.onChange(value);
                  // @ts-expect-error - dynamic field path
                  setValue(`${taskPath}.explore`, "");
                }}
              >
                <SelectTrigger className='w-full'>
                  <SelectValue
                    placeholder={selectedIntegration ? "Select model" : "Select integration first"}
                  />
                </SelectTrigger>
                <SelectContent>
                  {availableModels.map((model) => (
                    <SelectItem key={model} value={model}>
                      {model}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            )}
          />
          {taskErrors?.model && <FieldError>{taskErrors.model.message}</FieldError>}
        </div>

        <div className='space-y-2'>
          <Label htmlFor={`${taskPath}.explore`}>Explore</Label>
          <Controller
            control={control}
            // @ts-expect-error - dynamic field path
            name={`${taskPath}.explore`}
            rules={{ required: "Explore is required" }}
            render={({ field }) => (
              <Select
                value={(field.value as string) || ""}
                disabled={!selectedModel}
                onValueChange={(value) => {
                  field.onChange(value);
                  // @ts-expect-error - dynamic field path
                  setValue(`${taskPath}.fields`, []);
                  // @ts-expect-error - dynamic field path
                  setValue(`${taskPath}.filters`, []);
                  // @ts-expect-error - dynamic field path
                  setValue(`${taskPath}.sorts`, []);
                }}
              >
                <SelectTrigger className='w-full'>
                  <SelectValue
                    placeholder={selectedModel ? "Select explore" : "Select model first"}
                  />
                </SelectTrigger>
                <SelectContent>
                  {availableExplores.map((explore) => (
                    <SelectItem key={explore.name} value={explore.name}>
                      {explore.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            )}
          />
          {taskErrors?.explore && <FieldError>{taskErrors.explore.message}</FieldError>}
        </div>
      </div>

      <div className='space-y-2'>
        <div className='flex items-center justify-between'>
          <Label>Fields</Label>
          <Button
            type='button'
            onClick={() => appendField("" as never)}
            variant='outline'
            size='sm'
          >
            <Plus className='mr-1 h-4 w-4' />
            Add Field
          </Button>
        </div>
        {fieldEntries.length > 0 && (
          <div className='space-y-2'>
            {fieldEntries.map((field, fieldIndex) => (
              <div key={field.id} className='flex items-start gap-2'>
                <div className='min-w-0 flex-1'>
                  <Input
                    placeholder='view.field_name'
                    // @ts-expect-error - dynamic field path
                    {...register(`${taskPath}.fields.${fieldIndex}`)}
                  />
                </div>
                <Button
                  type='button'
                  onClick={() => removeField(fieldIndex)}
                  variant='ghost'
                  size='sm'
                >
                  <X className='h-4 w-4' />
                </Button>
              </div>
            ))}
          </div>
        )}
        <p className='text-muted-foreground text-sm'>
          Use the full field name, as in the Looker explore: view.field_name
        </p>
      </div>

      <div className='space-y-2'>
        <div className='flex items-center justify-between'>
          <Label>Filters</Label>
          <Button
            type='button'
            onClick={() => appendFilter({ key: "", value: "" } as never)}
            variant='outline'
            size='sm'
          >
            <Plus className='mr-1 h-4 w-4' />
            Add Filter
          </Button>
        </div>
        {filterEntries.length > 0 && (
          <div className='space-y-2'>
            {filterEntries.map((filter, filterIndex) => (
              <div key={filter.id} className='flex items-start gap-2'>
                <div className='min-w-0 flex-1'>
                  <Input
                    placeholder='view.field_name'
                    // @ts-expect-error - dynamic field path
                    {...register(`${taskPath}.filters.${filterIndex}.key`)}
                  />
                </div>
                <div className='flex-1'>
                  <Input
                    placeholder='Filter expression'
                    // @ts-expect-error - dynamic field path
                    {...register(`${taskPath}.filters.${filterIndex}.value`)}
                  />
                </div>
                <Button
                  type='button'
                  onClick={() => removeFilter(filterIndex)}
                  variant='ghost'
                  size='sm'
                >
                  <X className='h-4 w-4' />
                </Button>
              </div>
            ))}
          </div>
        )}
        <p className='text-muted-foreground text-sm'>
          Filter conditions as field name to Looker filter expression mappings
        </p>
      </div>

      <div className='space-y-2'>
        <Label htmlFor={`${taskPath}.filter_expression`}>Filter Expression</Label>
        <Input
          id={`${taskPath}.filter_expression`}
          placeholder='Optional Looker filter expression for complex conditions'
          // @ts-expect-error - dynamic field path
          {...register(`${taskPath}.filter_expression`)}
        />
      </div>

      <div className='space-y-2'>
        <div className='flex items-center justify-between'>
          <Label>Sort Fields</Label>
          <Button
            type='button'
            onClick={() => appendSort({ field: "", direction: "asc" } as never)}
            variant='outline'
            size='sm'
          >
            <Plus className='mr-1 h-4 w-4' />
            Add Sort
          </Button>
        </div>
        {sortEntries.length > 0 && (
          <div className='space-y-2'>
            {sortEntries.map((sort, sortIndex) => (
              <div key={sort.id} className='flex items-start gap-2'>
                <div className='min-w-0 flex-1'>
                  <Input
                    placeholder='view.field_name'
                    // @ts-expect-error - dynamic field path
                    {...register(`${taskPath}.sorts.${sortIndex}.field`)}
                  />
                </div>
                <div className='w-24'>
                  <Controller
                    control={control}
                    // @ts-expect-error - dynamic field path
                    name={`${taskPath}.sorts.${sortIndex}.direction`}
                    render={({ field: dirField }) => (
                      <Select
                        value={(dirField.value as string) || "asc"}
                        onValueChange={dirField.onChange}
                      >
                        <SelectTrigger className='w-full'>
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectItem value='asc'>Asc</SelectItem>
                          <SelectItem value='desc'>Desc</SelectItem>
                        </SelectContent>
                      </Select>
                    )}
                  />
                </div>
                <Button
                  type='button'
                  onClick={() => removeSort(sortIndex)}
                  variant='ghost'
                  size='sm'
                >
                  <X className='h-4 w-4' />
                </Button>
              </div>
            ))}
          </div>
        )}
        <p className='text-muted-foreground text-sm'>
          Sort fields are populated from synced metadata with ascending/descending options
        </p>
      </div>

      <div className='space-y-2'>
        <Label htmlFor={`${taskPath}.limit`}>Limit</Label>
        <Input
          id={`${taskPath}.limit`}
          type='number'
          min='-1'
          placeholder='Optional row limit (-1 for unlimited)'
          // @ts-expect-error - dynamic field path
          {...register(`${taskPath}.limit`, {
            valueAsNumber: true
          })}
        />
      </div>
    </div>
  );
};
