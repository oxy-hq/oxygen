import { useQuery } from "@tanstack/react-query";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { type AppIntegration, AppIntegrationsService } from "@/services/api/appIntegrations";
import queryKeys from "../queryKey";

export function useAppIntegrations() {
  const { project, branchName } = useCurrentProjectBranch();
  return useQuery<AppIntegration[]>({
    queryKey: queryKeys.appIntegrations.list(project.id, branchName),
    queryFn: () => AppIntegrationsService.list(project.id),
    staleTime: 30_000
  });
}
