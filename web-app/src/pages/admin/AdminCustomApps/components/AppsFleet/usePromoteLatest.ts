import { useMemo, useState } from "react";
import { toast } from "sonner";
import { useBatchPromoteLatestApps } from "@/hooks/api/customApps/useBatchAppMutations";
import { errMessage } from "@/hooks/api/errMessage";
import type { CustomApp } from "@/types/apps";

/**
 * The fleet's one verb: promote the newest build on every published app in view.
 *
 * **Published apps only.** On a draft the same call would be a first publish, and whether
 * an app goes live at all is the per-app card's decision, never a fleet-wide one. So
 * `promotable` is what the button counts and what the confirm names — not `rows.length`,
 * which would offer to promote drafts and then quietly not.
 *
 * In a hook because it is a mutation with its own state and its own result handling, and
 * the house rule is that a mutation beside rendering logic moves out. It also took the
 * try/catch and the partial-failure branch out of a component that was already the
 * densest on this surface.
 */
export function usePromoteLatest(rows: readonly CustomApp[]) {
  const promoteLatest = useBatchPromoteLatestApps();
  const [open, setOpen] = useState(false);
  const promotable = useMemo(() => rows.filter((a) => a.published_at), [rows]);

  const run = async () => {
    try {
      const result = await promoteLatest.mutateAsync(promotable.map((a) => a.id));
      // A batch that partly failed is not a success. It says both numbers, because
      // "promoted 9" over a fleet of ten hides the one that matters.
      if (result.failed > 0) {
        toast.warning(`Promoted ${result.succeeded}, ${result.failed} failed.`);
      } else {
        toast.success(
          `Promoted the latest build on ${result.succeeded} app${result.succeeded === 1 ? "" : "s"}.`
        );
      }
      setOpen(false);
    } catch (err) {
      toast.error(errMessage(err, "Couldn't promote the latest builds"));
    }
  };

  return { promotable, open, setOpen, run, pending: promoteLatest.isPending };
}
