import { useQuery } from "@tanstack/react-query";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import useFactoryAccess from "@/hooks/useFactoryAccess";
import { FileService } from "@/services/api";
import type { FileTreeResponse } from "@/types/file";
import queryKeys from "../queryKey";

export default function useFileTree(
  enabled = true,
  refetchOnWindowFocus = true,
  refetchOnMount: boolean | "always" = false
) {
  const { project, branchName } = useCurrentProjectBranch();
  // `/files` is the Factory's; chat and the thread page ask for the tree too.
  const factoryAccess = useFactoryAccess();

  return useQuery<FileTreeResponse>({
    queryKey: queryKeys.file.tree(project.id, branchName),
    queryFn: () => FileService.getFileTree(project.id, branchName),
    enabled: enabled && factoryAccess,
    refetchOnWindowFocus: refetchOnWindowFocus,
    refetchOnMount
  });
}
