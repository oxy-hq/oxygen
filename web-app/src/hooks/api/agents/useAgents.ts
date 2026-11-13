import { useQuery } from "@tanstack/react-query";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { AgentService } from "@/services/api";
import queryKeys from "../queryKey";

export default function useAgents() {
  const { project, branchName } = useCurrentProjectBranch();
  return useQuery({
    queryKey: queryKeys.agent.list(project.id, branchName),
    queryFn: () => AgentService.listAgents(project.id)
  });
}
