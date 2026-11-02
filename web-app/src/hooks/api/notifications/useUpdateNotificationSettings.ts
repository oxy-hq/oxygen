import { useMutation, useQueryClient } from "@tanstack/react-query";
import { isAxiosError } from "axios";
import { toast } from "sonner";
import { NotificationsService } from "@/services/api/notifications";
import type { NotificationSettings, NotificationSettingsPatch } from "@/types/notifications";
import queryKeys from "../queryKey";
import { refetchCatalog } from "./catalogUnread";
import {
  applySettingsPatch,
  invertSettingsPatch,
  settingsPatchApplies,
  settingsPatchChanges
} from "./settingsPatch";

const MUTATION_KEY = [...queryKeys.notifications.settings(), "update"] as const;

/** Settings changed elsewhere and this change no longer applies; `current` is what the server has. */
class SettingsConflictError extends Error {
  constructor(readonly current: NotificationSettings) {
    super("Notification settings changed elsewhere");
    this.name = "SettingsConflictError";
  }
}

/** The current document a 412 answers with, or `null` for any other failure. */
function staleDocument(err: unknown): NotificationSettings | null {
  if (!isAxiosError(err) || err.response?.status !== 412) return null;
  const data = err.response.data as Partial<NotificationSettings> | undefined;
  return data && typeof data.version === "string" ? (data as NotificationSettings) : null;
}

/**
 * Send `patch` against `version`. Refused as stale (412), it is checked
 * against the document the server answered with: still meaningful, it is sent
 * once more against that version; already true there, that document is the
 * answer; otherwise it is a conflict.
 */
async function saveSettings(
  patch: NotificationSettingsPatch,
  version: string | undefined
): Promise<NotificationSettings> {
  try {
    return await NotificationsService.updateSettings(patch, version);
  } catch (err) {
    const current = staleDocument(err);
    if (!current) throw err;
    if (!settingsPatchApplies(current, patch)) throw new SettingsConflictError(current);
    if (!settingsPatchChanges(current, patch)) return current;
    try {
      return await NotificationsService.updateSettings(patch, current.version);
    } catch (retryErr) {
      const latest = staleDocument(retryErr);
      throw latest ? new SettingsConflictError(latest) : retryErr;
    }
  }
}

/**
 * Change pause, quiet hours, or any node's settings. Optimistic: the switch
 * moves when pressed and only this change is undone if the server refuses.
 * Writes run one at a time, each against the version the last one left, so
 * quick taps do not refuse each other as stale.
 */
export const useUpdateNotificationSettings = () => {
  const qc = useQueryClient();
  const key = queryKeys.notifications.settings();
  /** Another write is queued behind this one (this one still counts as running). */
  const othersPending = () => qc.isMutating({ mutationKey: MUTATION_KEY }) > 1;

  return useMutation({
    mutationKey: MUTATION_KEY,
    scope: { id: MUTATION_KEY.join(":") },
    mutationFn: (patch: NotificationSettingsPatch) =>
      saveSettings(patch, qc.getQueryData<NotificationSettings>(key)?.version),
    onMutate: async (patch) => {
      await qc.cancelQueries({ queryKey: key });
      const before = qc.getQueryData<NotificationSettings>(key);
      if (!before) return { undo: null };
      qc.setQueryData(key, applySettingsPatch(before, patch));
      return { undo: invertSettingsPatch(before, patch) };
    },
    onSuccess: (doc) => {
      // A queued write's optimistic state stays on screen; it takes only the version.
      if (othersPending()) {
        qc.setQueryData<NotificationSettings>(
          key,
          (old) => old && { ...old, version: doc.version }
        );
      } else {
        qc.setQueryData(key, doc);
      }
    },
    onError: (err, _patch, context) => {
      if (err instanceof SettingsConflictError) {
        qc.setQueryData(key, err.current);
        toast.error("Settings changed elsewhere, so that wasn't saved");
        return;
      }
      const undo = context?.undo;
      if (undo) {
        qc.setQueryData<NotificationSettings>(key, (old) => old && applySettingsPatch(old, undo));
      }
      toast.error("Couldn't save that setting");
    },
    onSettled: () => {
      // The bell and the notifications page draw mutes from the catalogue.
      if (!othersPending()) refetchCatalog(qc);
    }
  });
};
