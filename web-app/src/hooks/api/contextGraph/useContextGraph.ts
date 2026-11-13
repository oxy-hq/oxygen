import { useQuery } from "@tanstack/react-query";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { ContextGraphService } from "@/services/api/contextGraph";
import queryKeys from "../queryKey";

/**
 * The context graph of the workspace's served (compiled) revision. A pinned
 * preview answers from its own revision (via the request header), so its
 * branch label is part of the key and never shares a cache entry with live.
 */
const useContextGraph = () => {
  const { project, branchName } = useCurrentProjectBranch();

  return useQuery({
    queryKey: queryKeys.contextGraph.graph(project.id, branchName),
    queryFn: () => ContextGraphService.getContextGraph(project.id)
  });
};

export default useContextGraph;
