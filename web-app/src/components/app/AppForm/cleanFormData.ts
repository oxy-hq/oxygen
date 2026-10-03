import { preserveTaskKeys } from "@/components/automation/AutomationForm/cleanFormData";
import { type CleanOptions, cleanObject } from "@/utils/formDataCleaner";
import type { AppFormData } from "./index";

/**
 * Display keys where empty and missing differ (`AppConfig` in
 * `crates/core/src/config/model.rs`, which the app's run parses strictly): a
 * row's `children` and a markdown block's `content` are required, so a missing
 * one fails the whole app, where `[]` and `""` render an empty block; and a
 * control's `default: ""` renders as `''` where a missing default renders as
 * none. The form only holds `default: ""` when the file did (see
 * `ControlDisplayFields`).
 */
const preserveDisplayKeys: NonNullable<CleanOptions["preserve"]> = (key, value) => {
  if (key === "children" && Array.isArray(value)) return "keep";
  if ((key === "content" || key === "default") && value === "") return "keep";
  return undefined;
};

const cleanEach = <T>(items: T[], preserve: CleanOptions["preserve"]): T[] =>
  items
    .map((item) => cleanObject(item as Record<string, unknown>, { preserve }) as T | null)
    .filter((item): item is T => item !== null);

/**
 * What the editor writes for the form's two keys. Both are required lists, so
 * an empty one is written as `[]`: the editor deletes a key this leaves out,
 * and an app without `tasks` or `display` does not parse.
 */
export const cleanAppFormData = (data: Partial<AppFormData>): Partial<AppFormData> => {
  const cleaned: Partial<AppFormData> = {};
  if (Array.isArray(data.tasks)) cleaned.tasks = cleanEach(data.tasks, preserveTaskKeys);
  if (Array.isArray(data.display)) cleaned.display = cleanEach(data.display, preserveDisplayKeys);
  return cleaned;
};
