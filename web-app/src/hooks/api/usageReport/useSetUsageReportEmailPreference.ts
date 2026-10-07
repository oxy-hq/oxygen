import { useMutation, useQueryClient } from "@tanstack/react-query";
import { UsageReportService } from "@/services/api/usageReport";
import {
  readEmailCaches,
  refreshEmailCaches,
  restoreEmailCaches,
  showOwnPreference,
  showRecipients
} from "./emailCaches";

interface SetInput {
  enabled: boolean;
}

/**
 * Turn the Monday email on or off for the signed-in staff member.
 *
 * Optimistic, so the switch moves when it is pressed, and rolled back if the server
 * refuses — a switch left showing a choice that was never saved is worse than a slow one.
 *
 * The caller's row in the list of recipients is this same setting, so it moves too.
 */
export const useSetUsageReportEmailPreference = () => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ enabled }: SetInput) => UsageReportService.setEmailPreference(enabled),
    onMutate: async ({ enabled }) => {
      const previous = await readEmailCaches(qc);
      showOwnPreference(qc, previous.own, enabled);
      showRecipients(qc, previous.list, (r) => r.is_self, enabled);
      return { previous };
    },
    onError: (_err, _vars, context) => {
      restoreEmailCaches(qc, context?.previous);
    },
    onSettled: () => {
      refreshEmailCaches(qc);
    }
  });
};
