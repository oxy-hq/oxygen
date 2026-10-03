import { Plus } from "lucide-react";
import type React from "react";
import { useEffect } from "react";
import { FormProvider, useFieldArray, useForm } from "react-hook-form";
import { NestedTasksForm } from "@/components/automation/AutomationForm/TasksForm/NestedTasksForm";
import { Button } from "@/components/ui/shadcn/button";
import { CardTitle } from "@/components/ui/shadcn/card";
import { cleanAppFormData } from "./cleanFormData";
import { DisplayForm } from "./DisplayForm";

export interface AppFormData {
  tasks?: TaskFormData[];
  display?: DisplayFormData[];
}

interface TaskFormData {
  name?: string;
  type?: string;
  cache?: {
    enabled?: boolean;
    path?: string;
  };
  export?: {
    format?: string;
    path?: string;
  };
  [key: string]: unknown;
}

interface DisplayFormData {
  type?: string;
  [key: string]: unknown;
}

interface AppFormProps {
  data?: Partial<AppFormData>;
  onChange?: (data: Partial<AppFormData>) => void;
}

const getDefaultData = (data?: Partial<AppFormData>) => {
  if (!data) {
    return {
      tasks: [{ name: "task_1", type: "execute_sql" }],
      display: [{ type: "table" }]
    };
  }

  const result: Partial<AppFormData> = {};

  if (data.tasks && Array.isArray(data.tasks) && data.tasks.length > 0) {
    result.tasks = data.tasks;
  }

  if (data.display && Array.isArray(data.display) && data.display.length > 0) {
    result.display = data.display;
  }

  return result;
};

export const AppForm: React.FC<AppFormProps> = ({ data, onChange }) => {
  const methods = useForm<AppFormData>({
    defaultValues: getDefaultData(data),
    mode: "onBlur"
  });

  const { subscribe } = methods;

  useEffect(() => {
    const callback = subscribe({
      formState: {
        values: true,
        isDirty: true
      },
      callback: ({ values, isDirty }) => {
        if (isDirty) {
          const cleaned = cleanAppFormData(values as Partial<AppFormData>);
          onChange?.(cleaned);
        }
      }
    });
    return () => callback();
  }, [subscribe, onChange]);

  const { control } = methods;

  const {
    fields: displayFields,
    append: appendDisplay,
    remove: removeDisplay
  } = useFieldArray({
    control,
    name: "display"
  });

  return (
    <FormProvider {...methods}>
      <div className='flex min-h-0 flex-1 flex-col'>
        <div className='flex-1 overflow-auto p-4'>
          <form id='app-form' className='space-y-8'>
            <NestedTasksForm
              label={<CardTitle>Tasks</CardTitle>}
              name='tasks'
              showAddButton={true}
            />

            <div className='flex items-center justify-between'>
              <CardTitle>Display</CardTitle>
              <Button
                type='button'
                onClick={() =>
                  appendDisplay({
                    type: "table"
                  })
                }
                variant='outline'
                size='sm'
              >
                <Plus />
                Add Display
              </Button>
            </div>
            <div className='space-y-4'>
              {displayFields.map((field, index) => (
                <div key={field.id}>
                  <DisplayForm index={index} onRemove={() => removeDisplay(index)} />
                </div>
              ))}
            </div>
          </form>
        </div>
      </div>
    </FormProvider>
  );
};
