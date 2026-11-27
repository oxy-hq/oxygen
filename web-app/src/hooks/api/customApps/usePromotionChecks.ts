import { useMutation, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { CustomAppsService } from "@/services/api/customApps";
import type { PromotionChecksUpdate } from "@/types/apps";
import { errMessage } from "../errMessage";
import queryKeys from "../queryKey";

/**
 * Save which checks hold an app's Promote and what follows one. The console reads the
 * app from the admin list, so every custom-app read is refreshed (the open promote
 * plan among them); the server audits the change.
 */
export const useSetPromotionChecks = (appId: string) => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: PromotionChecksUpdate) => CustomAppsService.setPromotionChecks(appId, body),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: queryKeys.customApps.all() });
      toast.success("Promotion checks saved");
    },
    onError: (err) => toast.error(errMessage(err, "Failed to save the promotion checks"))
  });
};
