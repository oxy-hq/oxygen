import { useQuery } from "@tanstack/react-query";
import { isAxiosError } from "axios";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { RevisionFilesService } from "@/services/api/revisionFiles";
import queryKeys from "../queryKey";

/** A `503`: the workspace has nothing compiled yet (retryable), not a failure. */
export const isNothingCompiled = (error: unknown) =>
  isAxiosError(error) && error.response?.status === 503;

/** While nothing is compiled, how often to look again for the first compile. */
const FIRST_COMPILE_POLL_MS = 30_000;

/** The revision this workspace's requests read (a pinned preview's, when pinned). */
const useServedRevision = () => {
  const { project, branchName } = useCurrentProjectBranch();
  return useQuery({
    queryKey: queryKeys.revisionFiles.served(project.id, branchName),
    queryFn: () => RevisionFilesService.served(project.id),
    retry: false,
    refetchInterval: (query) =>
      isNothingCompiled(query.state.error) ? FIRST_COMPILE_POLL_MS : false
  });
};

export default useServedRevision;
