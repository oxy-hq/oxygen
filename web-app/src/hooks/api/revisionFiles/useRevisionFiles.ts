import { useQuery } from "@tanstack/react-query";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { RevisionFilesService } from "@/services/api/revisionFiles";
import queryKeys from "../queryKey";

/** The served revision's files, sorted by path. */
const useRevisionFiles = () => {
  const { project, branchName } = useCurrentProjectBranch();
  return useQuery({
    queryKey: queryKeys.revisionFiles.list(project.id, branchName),
    queryFn: () => RevisionFilesService.list(project.id)
  });
};

export default useRevisionFiles;
