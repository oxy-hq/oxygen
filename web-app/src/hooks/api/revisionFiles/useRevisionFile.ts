import { useQuery } from "@tanstack/react-query";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { RevisionFilesService } from "@/services/api/revisionFiles";
import queryKeys from "../queryKey";

/** One file of the served revision; idle until a path is chosen. */
const useRevisionFile = (path: string | null) => {
  const { project, branchName } = useCurrentProjectBranch();
  return useQuery({
    queryKey: queryKeys.revisionFiles.file(project.id, branchName, path ?? ""),
    queryFn: () => RevisionFilesService.get(project.id, path ?? ""),
    enabled: !!path
  });
};

export default useRevisionFile;
