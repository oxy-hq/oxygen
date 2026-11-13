import { useQuery } from "@tanstack/react-query";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { DatabaseService } from "@/services/api";
import type { DatabaseInfo } from "@/types/database";
import queryKeys from "../queryKey";

export default function useDatabases(
  enabled = true,
  refetchOnWindowFocus = false,
  refetchOnMount: boolean | "always" = false
) {
  const { project, branchName } = useCurrentProjectBranch();
  const projectId = project.id;
  return useQuery<DatabaseInfo[], Error>({
    queryKey: queryKeys.database.list(projectId, branchName),
    queryFn: () => DatabaseService.listDatabases(projectId),
    enabled,
    refetchOnWindowFocus: refetchOnWindowFocus,
    refetchOnMount
  });
}
