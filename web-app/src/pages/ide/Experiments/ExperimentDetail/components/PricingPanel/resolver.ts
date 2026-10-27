import type { FieldErrors, Resolver } from "react-hook-form";
import { type PriceFormContext, type PriceFormValues, validatePriceForm } from "./priceForm";

/**
 * Plugs the pure validator into React Hook Form (`zod` is not a web-app
 * dependency, see Contract issues). Row errors land at `candidates[i].<field>`;
 * the set-level error lands at `candidates.root`, where a field array expects it.
 */
export function makePriceResolver(context: PriceFormContext): Resolver<PriceFormValues> {
  return (values) => {
    const { candidateErrors, formError } = validatePriceForm(values, context);
    const rows = candidateErrors.map((fields) =>
      Object.fromEntries(
        Object.entries(fields).map(([name, message]) => [name, { type: "validate", message }])
      )
    );
    if (!formError && rows.every((row) => Object.keys(row).length === 0)) {
      return { values, errors: {} };
    }
    const candidates = Object.assign(
      rows,
      formError ? { root: { type: "validate", message: formError } } : {}
    );
    return { values: {}, errors: { candidates } as unknown as FieldErrors<PriceFormValues> };
  };
}
