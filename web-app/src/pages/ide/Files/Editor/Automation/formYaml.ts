import type { AutomationFormData, TaskFormData } from "@/components/automation/AutomationForm";

/**
 * Only a Looker query keeps its filters as a map (`{field: value}`), which the
 * form edits as rows. A semantic query's filters are already a list, of
 * `{field, op, value}`: run through the map conversion they had no `key`, came
 * out empty, and every save from the form deleted them, so the query ran
 * unfiltered.
 */
const hasFilterMap = (task: TaskFormData) => task.type === "looker_query";

// Convert filters from YAML map {key: value} to form array [{key, value}]
const filtersMapToArray = (filters: unknown): Array<{ key: string; value: string }> | undefined => {
  if (!filters || typeof filters !== "object" || Array.isArray(filters)) return undefined;
  return Object.entries(filters as Record<string, string>).map(([key, value]) => ({
    key,
    value
  }));
};

// Convert filters from form array [{key, value}] to YAML map {key: value}
const filtersArrayToMap = (filters: unknown): Record<string, string> | undefined => {
  if (!Array.isArray(filters) || filters.length === 0) return undefined;
  const map: Record<string, string> = {};
  for (const f of filters as Array<{ key?: string; value?: string }>) {
    if (f.key) map[f.key] = f.value ?? "";
  }
  return Object.keys(map).length > 0 ? map : undefined;
};

const transformTasksForForm = (tasks: TaskFormData[]): TaskFormData[] =>
  tasks.map((task) => {
    if (!hasFilterMap(task)) return task;
    const converted = filtersMapToArray(task.filters);
    return converted !== undefined ? { ...task, filters: converted } : task;
  });

const transformTasksForYaml = (tasks: TaskFormData[]): TaskFormData[] =>
  tasks.map((task) => {
    if (!hasFilterMap(task)) return task;
    const { filters, ...rest } = task;
    const converted = filtersArrayToMap(filters);
    return converted === undefined ? rest : { ...rest, filters: converted };
  });

/** The top-level keys the form edits. Every other key is the file's own. */
const FORM_KEYS = ["name", "description", "tasks", "variables", "tests", "retrieval"] as const;

/**
 * The form edits `variables` as JSON text. Written back, it must be the map
 * again — as a string the file no longer parses (`variables` is a map). Text
 * that does not parse yet (mid-typing) keeps what the file had.
 */
const variablesForYaml = (variables: unknown, original: unknown): unknown => {
  if (typeof variables !== "string") return variables;
  try {
    return JSON.parse(variables);
  } catch {
    return original;
  }
};

/** The parsed file → the form's state. */
export const yamlToFormData = (
  parsed: Partial<AutomationFormData>
): Partial<AutomationFormData> => ({
  ...parsed,
  tasks: Array.isArray(parsed.tasks) ? transformTasksForForm(parsed.tasks) : parsed.tasks,
  variables:
    parsed.variables && typeof parsed.variables === "object"
      ? JSON.stringify(parsed.variables, null, 2)
      : parsed.variables?.toString() || ""
});

/**
 * The form's cleaned output → what is written to the file. Starts from the
 * file, so a top-level key the form has no field for (`consistency_prompt`,
 * `consistency_model`, anything newer) survives an edit; a key the form edits
 * is taken from the form, and removed when the form no longer has it.
 */
export const formDataToYaml = (
  formData: Partial<AutomationFormData>,
  original: Record<string, unknown> | undefined
): Record<string, unknown> => {
  const yaml: Record<string, unknown> = { ...original };
  for (const key of FORM_KEYS) {
    if (formData[key] === undefined) delete yaml[key];
    else yaml[key] = formData[key];
  }
  if (Array.isArray(formData.tasks)) yaml.tasks = transformTasksForYaml(formData.tasks);
  if (formData.variables !== undefined) {
    const variables = variablesForYaml(formData.variables, original?.variables);
    if (variables === undefined) delete yaml.variables;
    else yaml.variables = variables;
  }
  return yaml;
};
