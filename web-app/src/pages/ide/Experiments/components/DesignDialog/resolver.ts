import type { FieldErrors, Resolver } from "react-hook-form";
import type { DesignFormValues } from "./designForm";
import { type DesignErrors, toConfig } from "./toConfig";

/** Flat `waves.0.units` keys as the nested shape React Hook Form reads errors from. */
export function nestErrors(flat: DesignErrors): FieldErrors<DesignFormValues> {
  const root: Record<string, unknown> = {};
  for (const [path, message] of Object.entries(flat)) {
    const parts = path.split(".");
    let node: Record<string, unknown> = root;
    parts.forEach((part, i) => {
      if (i === parts.length - 1) {
        node[part] = { type: "validate", message };
        return;
      }
      if (node[part] === undefined) node[part] = /^\d+$/.test(parts[i + 1] ?? "") ? [] : {};
      node = node[part] as Record<string, unknown>;
    });
  }
  return root as FieldErrors<DesignFormValues>;
}

/**
 * Plugs the pure converter into React Hook Form (`zod` is not a web-app
 * dependency): a field the converter refuses gets its message beside it.
 */
export const designResolver: Resolver<DesignFormValues, { nameLocked?: boolean }> = (
  values,
  context
) => {
  const { errors } = toConfig(values, { checkName: !context?.nameLocked });
  if (Object.keys(errors).length === 0) return { values, errors: {} };
  return { values: {}, errors: nestErrors(errors) };
};
