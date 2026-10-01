import { useEffect, useState } from "react";
import { toast } from "sonner";
import { errMessage } from "@/hooks/api/errMessage";
import {
  useCreatePreview,
  usePreviews,
  useRefreshPreview
} from "@/hooks/api/workspaces/usePreviews";
import { cannotCompileMessage, isPreviewNotFound } from "@/libs/utils/preview";
import useSettingsDialog from "@/stores/useSettingsDialog";
import type { WorkspacePreview } from "@/types/workspace";

/** idle → requesting (reading the list / asking for a compile) → waiting (polling) → idle. */
export type OpenPreviewPhase = "idle" | "requesting" | "waiting";

/**
 * "Open preview" from the IDE: make sure a current preview of `branch` exists,
 * wait for it to compile, then hand the ready row — its revision is what gets
 * pinned — to `onReady`.
 *
 * What exists decides the first step — a ready preview opens at once; a
 * compiling one is waited on; a missing one is created; a failed or stale one
 * is recompiled (opening it would show the failure, or a revision the branch
 * has already moved past). Waiting rides the list's own polling, which runs
 * only while something is compiling, so the list is not read at all until the
 * button is pressed.
 */
export function useOpenPreview(
  workspaceId: string,
  branch: string,
  onReady: (preview: WorkspacePreview & { revision_id: string }) => void
) {
  const [phase, setPhase] = useState<OpenPreviewPhase>("idle");
  /** Why the last attempt could not compile, in the server's words. */
  const [notice, setNotice] = useState<string | null>(null);
  const previews = usePreviews(workspaceId, phase !== "idle");
  const create = useCreatePreview(workspaceId);
  const refresh = useRefreshPreview(workspaceId);
  const openSettings = useSettingsDialog((s) => s.open);

  const row = previews.data?.find((p) => p.branch === branch);

  const settle = (preview: WorkspacePreview | undefined) => {
    if (preview?.status === "ready" && preview.revision_id) {
      setPhase("idle");
      onReady({ ...preview, revision_id: preview.revision_id });
    } else if (preview?.status === "failed") {
      setPhase("idle");
      toast.error(`The preview of ${branch} failed to compile.`, {
        description: preview.error ?? undefined,
        action: { label: "See previews", onClick: () => openSettings("workspace.previews") }
      });
    } else if (preview?.status === "stale") {
      // The branch moved again while this compiled. Nothing polls a stale row,
      // so waiting on would wait forever — stop, and let the next press
      // recompile.
      setPhase("idle");
      toast.error(`${branch} moved while its preview compiled. Open preview again to recompile.`);
    } else {
      setPhase("waiting");
    }
  };

  const start = async () => {
    if (phase !== "idle") return;
    setNotice(null);
    setPhase("requesting");
    const { data: list, error } = await previews.refetch();
    if (!list) {
      setPhase("idle");
      toast.error(errMessage(error, "Couldn't load the workspace's previews."));
      return;
    }
    const current = list.find((p) => p.branch === branch);
    if (current && (current.status === "ready" || current.status === "compiling")) {
      settle(current);
      return;
    }
    // A refresh that comes back already `ready` (the head did not move) opens
    // at once via `settle` — it never passes through "Compiling preview…".
    (current ? refresh : create).mutate(branch, { onSuccess: settle, onError: failed });
  };

  const failed = (err: unknown) => {
    // The row vanished between the list and the refresh: this button's job is
    // "create it if needed", so create it rather than report a refresh of
    // nothing.
    if (isPreviewNotFound(err)) {
      create.mutate(branch, { onSuccess: settle, onError: failed });
      return;
    }
    setPhase("idle");
    // Uncommitted edits in the worktree, or no checkout: the server's words,
    // beside the button. Anything else the mutation hook has already toasted.
    const message = cannotCompileMessage(err);
    if (message) setNotice(message);
  };

  // While waiting, every poll of the list lands here.
  // biome-ignore lint/correctness/useExhaustiveDependencies: `settle` is recreated each render; the row and phase are what decide.
  useEffect(() => {
    if (phase === "waiting" && row && row.status !== "compiling") settle(row);
  }, [phase, row]);

  return { phase, start, notice };
}
