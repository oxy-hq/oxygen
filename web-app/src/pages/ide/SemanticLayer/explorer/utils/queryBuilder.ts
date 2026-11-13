import type {
  SemanticQueryFilter,
  SemanticQueryOrder,
  SemanticQueryRequest
} from "@/services/api/semantic";
import type { TimeDimension } from "@/types/artifact";
import type { Variable } from "../components/SemanticQueryPanel";

interface BuildSemanticQueryOptions {
  topic?: string;
  dimensions: string[];
  measures: string[];
  filters: SemanticQueryFilter[];
  orders: SemanticQueryOrder[];
  variables: Variable[];
  timeDimensions?: TimeDimension[];
  limit?: number;
}

/**
 * Builds a SemanticQueryRequest from the provided options.
 * Handles filter validation and variable transformation.
 */
export function buildSemanticQuery({
  topic,
  dimensions,
  measures,
  filters,
  orders,
  variables,
  timeDimensions,
  limit
}: BuildSemanticQueryOptions): SemanticQueryRequest {
  const processedFilters = filters
    .filter((f) => {
      if ("value" in f) return f.value !== null && f.value !== "";
      if ("values" in f) return f.values && f.values.length > 0;
      // For date range filters, include if relative exists OR both from and to exist
      if ("from" in f || "to" in f) {
        return "from" in f && "to" in f && f.from && f.to;
      }
      return false;
    })
    .map((f) => {
      const field = f.field;

      if ("values" in f) {
        return { field, op: f.op, values: f.values };
      } else if (f.op === "in_date_range" || f.op === "not_in_date_range") {
        // Handle date range filters - include relative and/or from/to if they exist
        const result: any = { field, op: f.op };
        if ("from" in f && f.from) {
          result.from = f.from;
        }
        if ("to" in f && f.to) {
          result.to = f.to;
        }
        return result;
      }
      // Type assertion since we've narrowed out date range filters
      return { field, op: f.op, value: (f as any).value };
    });

  const processedVariables = variables.reduce(
    (acc, v) => {
      if (v.key) acc[v.key] = v.value;
      return acc;
    },
    {} as Record<string, unknown>
  );

  const processedOrders = orders.map((order) => ({
    field: order.field,
    direction: order.direction
  }));

  // Separate time dimensions based on granularity
  const timeDimensionsWithValue: string[] = [];
  const processedTimeDimensions = timeDimensions
    ?.filter((td) => td.dimension) // Only include time dimensions with a dimension field
    .filter((td) => {
      // If granularity is "value", add to dimensions instead
      if (td.granularity === "value") {
        timeDimensionsWithValue.push(td.dimension);
        return false;
      }
      return true;
    })
    .map((td) => ({
      dimension: td.dimension,
      ...(td.granularity && { granularity: td.granularity })
    }));

  return {
    query: {
      ...(topic && { topic }),
      dimensions: [...dimensions, ...timeDimensionsWithValue],
      measures: measures,
      filters: processedFilters,
      orders: processedOrders,
      variables: processedVariables,
      ...(processedTimeDimensions &&
        processedTimeDimensions.length > 0 && { time_dimensions: processedTimeDimensions }),
      ...(limit !== undefined && { limit })
    },
    result_format: "parquet"
  };
}
