import { type CleanOptions, cleanObject } from "@/utils/formDataCleaner";
import type { AutomationFormData } from "./index";

/**
 * Lists the backend requires (`crates/agentic/automation/src/config.rs`): a
 * missing one fails the whole file to parse, `[]` parses. An automation's or a
 * loop's `tasks`, a condition's `tasks` (a new condition starts with none), a
 * conditional's `conditions`, and a Looker or Omni query's `fields` — which an
 * app's strict parser requires too.
 */
const REQUIRED_LISTS = new Set(["tasks", "conditions", "fields"]);

/**
 * The user's own data rather than form fields, where "", `null` and `[]`
 * inside are values: variable declarations (a `default: ""` stripped makes the
 * variable resolve to its whole declaration), a SQL task's variable overrides,
 * and loop and filter value lists (a stripped `""` or `null` item is a lost
 * iteration or a changed filter).
 */
const DATA_KEYS = new Set(["variables", "values"]);

/** Task keys where `cleanObject`'s stripping changes what the backend reads. */
export const preserveTaskKeys: NonNullable<CleanOptions["preserve"]> = (key, value) => {
  if (DATA_KEYS.has(key) && value !== null && typeof value === "object") return "verbatim";
  if (REQUIRED_LISTS.has(key) && Array.isArray(value)) return "keep";
  // A semantic filter's `value` is required and `null` means IS NULL. The form
  // never writes `null`, so this only carries the file's own through an edit.
  if (key === "value" && value === null) return "keep";
  return undefined;
};

export const cleanAutomationFormData = (
  data: Partial<AutomationFormData>
): Partial<AutomationFormData> =>
  (cleanObject(data as Record<string, unknown>, {
    preserve: preserveTaskKeys
  }) as Partial<AutomationFormData> | null) ?? {};
