import { useQuery } from "@tanstack/react-query";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { ChartService } from "@/services/api";
import queryKeys from "./queryKey";

export default function useChart(file_path: string, enabled = true) {
  const { project, branchName } = useCurrentProjectBranch();

  return useQuery({
    queryKey: queryKeys.chart.get(project.id, branchName, file_path),
    queryFn: () => ChartService.getChart(project.id, file_path),
    enabled
  });
}
