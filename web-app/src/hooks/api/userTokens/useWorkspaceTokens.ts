import { useQuery } from "@tanstack/react-query";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { apiStatus } from "@/libs/apiError";
import { WorkspaceTokenService } from "@/services/api/apiToken";
import type { WorkspaceTokenListResponse } from "@/types/apiToken";
import queryKeys from "../queryKey";

/** Every token that can reach the current workspace, whoever owns it. Read-only. */
const useWorkspaceTokens = () => {
  const { project } = useCurrentProjectBranch();
  const projectId = project.id;
  return useQuery<WorkspaceTokenListResponse, Error>({
    queryKey: queryKeys.apiKey.inventory(projectId),
    queryFn: () => WorkspaceTokenService.list(projectId),
    refetchOnWindowFocus: false,
    // A 404 is an answer (this server predates the inventory), not a blip worth retrying.
    retry: (failureCount, error) => apiStatus(error) !== 404 && failureCount < 2
  });
};

export default useWorkspaceTokens;
