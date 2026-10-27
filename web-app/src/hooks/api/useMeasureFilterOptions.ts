import { useQuery } from "@tanstack/react-query";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { encodeBase64 } from "@/libs/encoding";
import { SemanticService } from "@/services/api/semantic";
import queryKeys from "./queryKey";
import { useViewDetails } from "./useSemanticQuery";
import useViewFiles from "./useViewFiles";

/** How many distinct values a filter's value list offers. */
export const DIMENSION_VALUE_LIMIT = 500;

/** The dimensions of the view a `<view>.<measure>` is read from: what a filter on it may name. */
export function useMeasureDimensions(measure: string) {
  const view = measure.includes(".") ? measure.split(".")[0] : "";
  const { viewFiles, isLoading: filesLoading } = useViewFiles();
  const path = view ? viewFiles.find((f) => f.value === view)?.path : undefined;
  const { data, isLoading } = useViewDetails(path ? encodeBase64(path) : undefined);
  return {
    view,
    dimensions: data?.dimensions ?? [],
    isLoading: filesLoading || (Boolean(path) && isLoading)
  };
}

/** The values of a `[header, ...rows]` answer's first column, each once, in order. */
export function distinctValues(rows: unknown): string[] {
  if (!Array.isArray(rows)) return [];
  const seen = new Set<string>();
  for (const row of rows.slice(1)) {
    const v = Array.isArray(row) ? row[0] : undefined;
    if (typeof v === "string" && v !== "") seen.add(v);
  }
  return [...seen];
}

/**
 * The distinct values of one `<view>.<dimension>`, read from the warehouse.
 * Asked together with `measure` (the filtered measure, of the same view) so the
 * query groups by the dimension: a dimension alone returns raw rows. Largest first.
 */
export function useDimensionValues(dimension: string, measure: string) {
  const { project, branchName } = useCurrentProjectBranch();
  return useQuery({
    queryKey: queryKeys.semantic.dimensionValues(project.id, `${dimension}|${measure}`, branchName),
    queryFn: async () =>
      distinctValues(
        await SemanticService.executeSemanticQuery(
          project.id,
          {
            query: {
              dimensions: [dimension],
              measures: [measure],
              orders: [{ field: measure, direction: "desc" }],
              limit: DIMENSION_VALUE_LIMIT
            },
            result_format: "json"
          },
          branchName
        )
      ),
    enabled: dimension.includes(".") && measure.includes("."),
    staleTime: 5 * 60 * 1000,
    retry: false
  });
}
