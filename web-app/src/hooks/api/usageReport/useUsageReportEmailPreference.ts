import { useQuery } from "@tanstack/react-query";
import { UsageReportService } from "@/services/api/usageReport";
import queryKeys from "../queryKey";

/** Whether the signed-in staff member gets the Monday email, and how this deployment sends it. */
export const useUsageReportEmailPreference = () =>
  useQuery({
    queryKey: queryKeys.usageReport.emailPreference(),
    queryFn: () => UsageReportService.getEmailPreference()
  });
