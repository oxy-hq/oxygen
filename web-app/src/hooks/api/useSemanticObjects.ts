import { useQuery } from "@tanstack/react-query";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import useFactoryAccess from "@/hooks/useFactoryAccess";
import { SemanticService } from "@/services/api/semantic";
import queryKeys from "./queryKey";

/**
 * The views and topics of the revision the workspace serves.
 *
 * `GET /semantic/objects` is the Factory's (the semantic explorer). The chat
 * thread's semantic-query panel asks for it too, for field options; for a
 * customer the query is not sent and those options stay empty.
 */
export default function useSemanticObjects() {
  const { project, branchName } = useCurrentProjectBranch();
  const factoryAccess = useFactoryAccess();
  return useQuery({
    queryKey: queryKeys.semantic.objects(project.id, branchName),
    queryFn: () => SemanticService.listObjects(project.id, branchName),
    enabled: !!project.id && factoryAccess
  });
}
