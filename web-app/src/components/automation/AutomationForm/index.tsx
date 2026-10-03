import { Plus } from "lucide-react";
import type React from "react";
import { useEffect } from "react";
import { FormProvider, useFieldArray, useForm } from "react-hook-form";
import { TestsForm } from "@/components/shared/TestsForm";
import { Button } from "@/components/ui/shadcn/button";
import { CardTitle } from "@/components/ui/shadcn/card";
import { FieldError } from "@/components/ui/shadcn/field";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import { Textarea } from "@/components/ui/shadcn/textarea";
import { cleanAutomationFormData } from "./cleanFormData";
import { RetrievalForm } from "./RetrievalForm";
import { TasksForm } from "./TasksForm";
import { VariablesForm } from "./VariablesForm";

export interface AutomationFormData {
  name?: string;
  description?: string;
  tasks?: TaskFormData[];
  variables?: unknown;
  tests?: TestFormData[];
  retrieval?: RetrievalConfigData | null;
}

export interface TaskFormData {
  name?: string;
  type?: string;
  cache?: {
    enabled?: boolean;
    path?: string;
  };
  export?: {
    enabled?: boolean;
    format?: string;
    path?: string;
  };
  [key: string]: unknown;
}

interface TestFormData {
  type?: string;
  concurrency?: number;
  task_ref?: string;
  metrics?: unknown[];
  [key: string]: unknown;
}

interface RetrievalConfigData {
  include?: string[];
  exclude?: string[];
}

interface AutomationFormProps {
  data?: Partial<AutomationFormData>;
  onChange?: (data: Partial<AutomationFormData>) => void;
}

const getDefaultData = (data?: Partial<AutomationFormData>) => {
  if (!data) {
    return {
      name: "",
      description: "",
      tasks: [{ name: "task_1", type: "agent" }],
      variables: "{}",
      tests: [],
      retrieval: null
    };
  }

  const result: Partial<AutomationFormData> = {};

  if (data.name !== undefined) result.name = data.name;
  if (data.description !== undefined) result.description = data.description;
  if (data.variables !== undefined) result.variables = data.variables;

  if (data.tasks && Array.isArray(data.tasks) && data.tasks.length > 0) {
    result.tasks = data.tasks.map((task) => {
      const processedTask = { ...task };

      if (processedTask.export && typeof processedTask.export === "object") {
        if (processedTask.export.format || processedTask.export.path) {
          processedTask.export.enabled = true;
        }
      }

      return processedTask;
    });
  }

  if (data.tests && Array.isArray(data.tests) && data.tests.length > 0) {
    result.tests = data.tests;
  }

  if (data.retrieval && typeof data.retrieval === "object") {
    const hasInclude =
      data.retrieval.include &&
      Array.isArray(data.retrieval.include) &&
      data.retrieval.include.length > 0;
    const hasExclude =
      data.retrieval.exclude &&
      Array.isArray(data.retrieval.exclude) &&
      data.retrieval.exclude.length > 0;
    if (hasInclude || hasExclude) {
      result.retrieval = data.retrieval;
    }
  }

  console.log("Processed initial data:", result);
  return result;
};

export const AutomationForm: React.FC<AutomationFormProps> = ({ data, onChange }) => {
  const methods = useForm<AutomationFormData>({
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
          const cleaned = cleanAutomationFormData(values as Partial<AutomationFormData>);
          onChange?.(cleaned);
        }
      }
    });
    return () => callback();
  }, [subscribe, onChange]);

  const {
    control,
    register,
    formState: { errors }
  } = methods;

  const {
    fields: testFields,
    append: appendTest,
    remove: removeTest
  } = useFieldArray({
    control,
    name: "tests"
  });

  return (
    <FormProvider {...methods}>
      <div className='flex min-h-0 flex-1 flex-col'>
        <div className='flex-1 overflow-auto p-4'>
          <form id='automation-form' className='space-y-8'>
            <div className='space-y-4'>
              <div className='space-y-2'>
                <Label htmlFor='name'>Name</Label>
                <Input
                  id='name'
                  placeholder='Describe what this automation does...'
                  {...register("name")}
                />
                {errors.name && <FieldError>{errors.name.message}</FieldError>}
              </div>
              <div className='space-y-2'>
                <Label htmlFor='description'>Description</Label>
                <Textarea
                  id='description'
                  placeholder='Describe what this automation does...'
                  {...register("description")}
                  rows={4}
                />
                {errors.description && <FieldError>{errors.description.message}</FieldError>}
              </div>
            </div>

            <TasksForm />

            <VariablesForm />

            <div className='flex items-center justify-between'>
              <CardTitle>Tests</CardTitle>
              <Button
                type='button'
                onClick={() =>
                  appendTest({
                    type: "consistency",
                    concurrency: 10
                  })
                }
                variant='outline'
                size='sm'
              >
                <Plus />
                Add Test
              </Button>
            </div>
            <div className='space-y-4'>
              {testFields.map((field, index) => (
                <TestsForm<AutomationFormData>
                  key={field.id}
                  index={index}
                  onRemove={() => removeTest(index)}
                />
              ))}
            </div>

            <RetrievalForm />
          </form>
        </div>
      </div>
    </FormProvider>
  );
};
