import { useQuery } from "@tanstack/react-query";
import { UsageReportService } from "@/services/api/usageReport";
import queryKeys from "../queryKey";

/** The latest weekly report. `data.report` is `null` until the first one is written. */
export const useUsageReport = () =>
  useQuery({
    queryKey: queryKeys.usageReport.latest(),
    queryFn: () => UsageReportService.latest()
  });
