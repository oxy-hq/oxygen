import { useMutation, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { CustomAppsService } from "@/services/api/customApps";
import { errMessage } from "../errMessage";
import queryKeys from "../queryKey";

/**
 * Turn an app's four-eyes rule on or off. The console reads the app from the admin
 * list, so every custom-app read is refreshed; the server audits the change.
 */
export const useSetPromotionRule = (appId: string) => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (requireSecondPerson: boolean) =>
      CustomAppsService.setPromotionRule(appId, requireSecondPerson),
    onSuccess: (app) => {
      qc.invalidateQueries({ queryKey: queryKeys.customApps.all() });
      toast.success(
        app.require_second_person_to_promote
          ? `${app.name}: a second person now promotes each build`
          : `${app.name}: the publisher may promote their own build`
      );
    },
    onError: (err) => toast.error(errMessage(err, "Failed to change who may promote"))
  });
};
