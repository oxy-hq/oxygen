import { CardTitle } from "@/components/ui/shadcn/card";
import { NestedTasksForm } from "./NestedTasksForm";

export interface AutomationFormData {
  name?: string;
  description?: string;
  tasks?: TaskFormData[];
  variables?: string;
  tests?: TestFormData[];
  retrieval?: RetrievalConfigData | null;
}

interface TaskFormData {
  name?: string;
  type?: string;
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

export const TasksForm = () => {
  return <NestedTasksForm label={<CardTitle>Tasks</CardTitle>} name='tasks' showAddButton={true} />;
};
