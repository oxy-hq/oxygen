import { useQuery } from "@tanstack/react-query";
import { UsageReportService } from "@/services/api/usageReport";
import queryKeys from "../queryKey";

/**
 * Everyone the Monday report is addressed to.
 *
 * `enabled` is for the caller's standing. The endpoint needs `manage_platform_grants`,
 * which is narrower than the page this list sits on, so asking unconditionally is a 403
 * toast on every visit for someone who was never going to be shown the list.
 */
export const useUsageReportRecipients = (options: { enabled?: boolean } = {}) =>
  useQuery({
    queryKey: queryKeys.usageReport.recipients(),
    queryFn: () => UsageReportService.recipients(),
    enabled: options.enabled ?? true
  });
