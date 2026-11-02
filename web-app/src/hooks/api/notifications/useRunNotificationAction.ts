import { useMutation, useQueryClient } from "@tanstack/react-query";
import { isAxiosError } from "axios";
import { toast } from "sonner";
import { NotificationsService } from "@/services/api/notifications";
import type { InboxItem } from "@/types/notifications";
import { errMessage } from "../errMessage";
import { updateCatalogUnread, withItemRead } from "./catalogUnread";
import { invalidateInbox } from "./invalidateInbox";

export interface RunNotificationAction {
  item: InboxItem;
  actionId: string;
}

/**
 * What to say when an action did not run. A refusal of the app's function
 * (`403 FunctionRefused`) and any other say the server's own words.
 */
function actionErrorMessage(err: unknown): string {
  if (isAxiosError(err)) {
    const status = err.response?.status;
    if (status === 409) return "This action is no longer available";
    if (status === 404) return "This notification is gone";
  }
  return errMessage(err, "Couldn't do that");
}

/**
 * Run one of a notification's function actions as the signed-in person. The
 * server marks the notification read only when the function succeeded, so the
 * catalogue's counts drop then; the lists and the badge are re-read either way.
 */
export const useRunNotificationAction = () => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ item, actionId }: RunNotificationAction) =>
      NotificationsService.runAction(item.id, actionId),
    onSuccess: async (_result, { item }) => {
      toast.success("Done");
      await updateCatalogUnread(qc, (catalog) => withItemRead(catalog, item));
    },
    onError: (err) => {
      toast.error(actionErrorMessage(err));
    },
    onSettled: () => invalidateInbox(qc)
  });
};
