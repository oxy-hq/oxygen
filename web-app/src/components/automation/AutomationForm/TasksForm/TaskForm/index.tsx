import { ChevronDown, ChevronRight, Trash2 } from "lucide-react";
import type React from "react";
import { useState } from "react";
import { Controller, useFormContext } from "react-hook-form";
import { Button } from "@/components/ui/shadcn/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger
} from "@/components/ui/shadcn/collapsible";
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
import type { AutomationFormData } from "..";
import {
  AirwayTaskFields,
  AutomationTaskFields,
  ConditionalTaskFields,
  ExecuteSqlTaskFields,
  FormatterTaskFields,
  LookerQueryTaskFields,
  LoopSequentialTaskFields,
  OmniQueryTaskFields,
  SemanticQueryTaskFields
} from "./TaskFields";

interface TaskFormProps {
  index: number;
  onRemove: () => void;
  /**
   * Base path for the tasks array (e.g., "tasks" or "tasks.0.tasks")
   */
  basePath?: string;
}

const TASK_TYPES = [
  { value: "agent", label: "Agent" },
  { value: "execute_sql", label: "Execute SQL" },
  { value: "semantic_query", label: "Semantic Query" },
  { value: "looker_query", label: "Looker Query" },
  { value: "loop_sequential", label: "Loop Sequential" },
  { value: "formatter", label: "Formatter" },
  { value: "workflow", label: "Automation" },
  { value: "omni_query", label: "Omni Query" },
  { value: "conditional", label: "Conditional" },
  { value: "airway", label: "Airway" }
];

export const TaskForm: React.FC<TaskFormProps> = ({ index, onRemove, basePath = "tasks" }) => {
  const {
    register,
    control,
    watch,
    formState: { errors },
    setValue,
    getValues
  } = useFormContext<AutomationFormData>();

  const [isOpen, setIsOpen] = useState(false);

  // Build the full path for this task (e.g., "tasks.0" or "tasks.0.tasks.1")
  const taskPath = `${basePath}.${index}` as const;

  // @ts-expect-error - Dynamic path for nested tasks
  const taskType = watch(`${taskPath}.type`) as string | undefined;
  // @ts-expect-error - Dynamic path for nested tasks
  const taskName = watch(`${taskPath}.name`) as string | undefined;
  // @ts-expect-error - Dynamic path for nested tasks
  const taskErrors = errors[basePath]?.[index];

  const getTaskTypeLabel = (type: string) => {
    const taskTypeObj = TASK_TYPES.find((t) => t.value === type);
    return taskTypeObj?.label || type;
  };

  const onChangeTaskType = (newType: string) => {
    // @ts-expect-error - Dynamic path for nested tasks
    const task = getValues(taskPath) as Record<string, unknown>;
    if (newType !== taskType) {
      // @ts-expect-error - Dynamic path for nested tasks
      setValue(taskPath, {
        name: task?.name,
        type: task?.type
      });
    }
  };

  const renderTaskSpecificFields = () => {
    switch (taskType) {
      case "execute_sql":
        return <ExecuteSqlTaskFields index={index} basePath={basePath} />;

      case "airway":
        return <AirwayTaskFields index={index} basePath={basePath} />;

      case "semantic_query":
        return <SemanticQueryTaskFields index={index} basePath={basePath} />;

      case "looker_query":
        return <LookerQueryTaskFields index={index} basePath={basePath} />;

      case "omni_query":
        return <OmniQueryTaskFields index={index} basePath={basePath} />;

      case "formatter":
        return <FormatterTaskFields index={index} basePath={basePath} />;

      case "workflow":
        return <AutomationTaskFields index={index} basePath={basePath} />;

      case "loop_sequential":
        return <LoopSequentialTaskFields index={index} basePath={basePath} />;

      case "conditional":
        return <ConditionalTaskFields index={index} basePath={basePath} />;

      default:
        return null;
    }
  };

  return (
    <div className='rounded-lg border p-3'>
      <Collapsible open={isOpen} onOpenChange={setIsOpen}>
        <CollapsibleTrigger className='w-full rounded-lg'>
          <div className='flex min-w-0 items-center justify-between'>
            {isOpen ? (
              <ChevronDown className='h-5 w-5 text-muted-foreground' />
            ) : (
              <ChevronRight className='h-5 w-5 text-muted-foreground' />
            )}
            <div className='flex min-w-0 flex-1 items-center gap-3'>
              <span className='flex h-8 w-8 items-center justify-center rounded-full bg-primary/10 font-semibold text-primary text-sm'>
                {index + 1}
              </span>
              <div className='flex min-w-0 flex-1 items-center gap-2'>
                <span className='truncate font-medium text-sm'>{taskName || "Untitled Task"}</span>
                {taskType && (
                  <span className='rounded-md bg-muted px-2 py-1 text-muted-foreground text-xs'>
                    {getTaskTypeLabel(taskType)}
                  </span>
                )}
              </div>
            </div>
            <Button
              type='button'
              onClick={(e) => {
                e.stopPropagation();
                onRemove();
              }}
              variant='ghost'
              size='sm'
            >
              <Trash2 className='h-4 w-4' />
            </Button>
          </div>
        </CollapsibleTrigger>

        <CollapsibleContent className='mt-4 space-y-4'>
          <div className='space-y-4'>
            <div className='grid grid-cols-2 gap-4'>
              <div className='space-y-2'>
                <Label htmlFor={`${taskPath}.name`}>Task Name</Label>
                <Input
                  id={`${taskPath}.name`}
                  placeholder='Enter task name'
                  // @ts-expect-error - Dynamic path for nested tasks
                  {...register(`${taskPath}.name`, {
                    required: "Task name is required",
                    pattern: {
                      value: /^[a-zA-Z]\w*$/,
                      message:
                        "Name must start with a letter and contain only alphanumeric characters and underscores"
                    }
                  })}
                />
                {taskErrors?.name && <FieldError>{taskErrors.name.message}</FieldError>}
              </div>
              <div className='space-y-2'>
                <Label htmlFor={`${taskPath}.type`}>Task Type</Label>
                <Controller
                  // @ts-expect-error - Dynamic path for nested tasks
                  name={`${taskPath}.type`}
                  control={control}
                  rules={{ required: "Task type is required" }}
                  render={({ field }) => (
                    <Select
                      onValueChange={(value) => {
                        field.onChange(value);
                        onChangeTaskType(value);
                      }}
                      defaultValue={field.value as string}
                    >
                      <SelectTrigger>
                        <SelectValue placeholder='Select task type' />
                      </SelectTrigger>
                      <SelectContent>
                        {TASK_TYPES.map((type) => (
                          <SelectItem
                            className='cursor-pointer'
                            key={type.value}
                            value={type.value}
                          >
                            {type.label}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  )}
                />
                {taskErrors?.type && <FieldError>{taskErrors.type.message}</FieldError>}
              </div>
            </div>

            {renderTaskSpecificFields()}
          </div>
        </CollapsibleContent>
      </Collapsible>
    </div>
  );
};
