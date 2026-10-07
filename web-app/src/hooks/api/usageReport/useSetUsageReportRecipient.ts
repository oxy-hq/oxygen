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
  email: string;
  enabled: boolean;
}

/**
 * Turn the Monday email on or off for one named person — someone else, or the caller.
 *
 * Optimistic with rollback, like the caller's own switch. When the row is the caller's
 * own, their preference moves with it: the two are one setting shown twice.
 */
export const useSetUsageReportRecipient = () => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ email, enabled }: SetInput) => UsageReportService.setRecipient(email, enabled),
    onMutate: async ({ email, enabled }) => {
      const previous = await readEmailCaches(qc);
      showRecipients(qc, previous.list, (r) => r.email === email, enabled);
      // The list says which row is the caller's. Comparing addresses here would be a
      // second, weaker copy of something the server has already decided.
      const isOwnRow = previous.list?.recipients.some((r) => r.email === email && r.is_self);
      if (isOwnRow) showOwnPreference(qc, previous.own, enabled);
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
