import { useQuery } from "@tanstack/react-query";
import queryKeys from "@/hooks/api/queryKey";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { BuilderService } from "@/services/api";

/**
 * Hook to check if the builder agent is available.
 * Supports both legacy path-based agents and the new built-in copilot.
 * Uses React Query so all call sites share a single cached request.
 */
export default function useBuilderAvailable() {
  const { project } = useCurrentProjectBranch();
  const projectId = project?.id ?? "";

  const { data, isLoading, isError } = useQuery({
    queryKey: queryKeys.builder.availability(projectId),
    queryFn: () => BuilderService.checkBuilderAvailability(projectId),
    // Availability is stable for the lifetime of a project session.
    staleTime: 5 * 60 * 1000
  });

  const isAvailable = data?.available ?? false;
  const builderPath = data?.builder_path ?? "";
  const isBuiltin = data?.builtin ?? false;
  const builderModel = data?.model;

  return { isAvailable, isLoading, isError, builderPath, isBuiltin, builderModel };
}
