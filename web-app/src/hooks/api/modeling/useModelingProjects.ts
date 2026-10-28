import { useQuery } from "@tanstack/react-query";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import useFactoryAccess from "@/hooks/useFactoryAccess";
import { ModelingService } from "@/services/api/modeling";
import queryKeys from "../queryKey";

export default function useModelingProjects() {
  const { project, branchName } = useCurrentProjectBranch();
  const factoryAccess = useFactoryAccess();

  return useQuery({
    queryKey: queryKeys.modeling.projects(project.id, branchName),
    queryFn: () => ModelingService.listProjects(project.id, branchName),
    // `/modeling` is the Factory's: the builder dialog mounts for every page.
    enabled: factoryAccess
  });
}
