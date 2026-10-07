import { useMutation } from "@tanstack/react-query";
import { UsageReportService } from "@/services/api/usageReport";

/** Send the latest report to the signed-in staff member, whatever their Monday setting. */
export const useSendUsageReportToMe = () =>
  useMutation({
    mutationFn: () => UsageReportService.sendToMe()
  });
