import { useRef } from "react";
import { toast } from "sonner";
import { usePublishApp, useUnpublishApp } from "@/hooks/api/customApps/useCustomApps";
import { useMoveProduction } from "@/hooks/api/customApps/useProductionMove";
import { errMessage } from "@/hooks/api/errMessage";
import { useSetAppPublished } from "@/hooks/api/partners";
import type { MoveOverride, MovePlan, MoveTarget } from "@/types/appPromote";
import type { AppBuild } from "@/types/apps";
import { shortBuildId } from "./lifecycleCopy";
import { overrideRefusal } from "./planChecks";

/**
 * What a surface can *do* to an app's lifecycle. The card renders a control for each
 * verb present and nothing for one absent, so the partner console — which can publish
 * and unpublish but has no build history to roll back through — hands the same card a
 * narrower set rather than a different card.
 *
 * Every verb resolves when the server has answered, so the confirm can close on
 * success and stay open (with the error toasted) on failure.
 */
export interface LifecycleActions {
  /** Point the live channel at the draft build (`POST /customer-apps/{id}/publish`).
   *  `override` promotes past the build's checks with a written reason. */
  promoteDraft?: (override?: MoveOverride) => Promise<void>;
  /** First publish; same endpoint, read differently by an app with nothing live. */
  publish?: () => Promise<void>;
  /** Move the live pointer to a retained build. Only its `app_builds.id` is read, so
   *  a surface holding no `AppBuild` (the release timeline) names the id alone. */
  rollbackTo?: (build: Pick<AppBuild, "id">) => Promise<void>;
  unpublish?: () => Promise<void>;
  /** Any verb in flight — every control disables together. */
  pending: boolean;
}

/** A production move's dry-run plan, read for the build its confirm names. */
export interface PlannedMove {
  target: MoveTarget;
  plan: MovePlan;
}

/**
 * The staff console's verbs. Promote to production and roll back are production moves made
 * through the plan routes `oxyc promote` and `oxyc rollback` call, acting on the plan
 * the confirm showed (`planned`): they are refused on the same blockers, register and
 * retire schedules as those do, and refuse a plan production moved past since it was
 * read. A first publish and unpublish stay on `/customer-apps/{id}/publish`, which
 * also sets the app's published flag.
 *
 * The card stores a verb when its button is clicked, before the plan has loaded, and
 * runs it on Confirm. So a verb reads the plan through a ref at the moment it runs,
 * never the `planned` of the render that created it.
 */
export function useAdminLifecycleActions(appId: string, planned?: PlannedMove): LifecycleActions {
  const publish = usePublishApp();
  const unpublish = useUnpublishApp();
  const move = useMoveProduction(appId);
  const plannedRef = useRef(planned);
  plannedRef.current = planned;

  const act = async (
    act: MoveTarget["act"],
    buildId: string | null,
    failure: string,
    override?: MoveOverride
  ) => {
    const planned = plannedRef.current;
    const matches =
      planned?.target.act === act && (buildId === null || planned.target.buildId === buildId);
    if (!planned || !matches) {
      toast.error(`${failure}: the plan for this build has not been read`);
      throw new Error("no plan for this move");
    }
    try {
      await move.mutateAsync(override ? { ...planned, override } : planned);
      toast.success(`Build ${shortBuildId(planned.plan.build.build_id)} is now live.`);
    } catch (err) {
      // A refused override is said under the reason it was written with, not toasted.
      if (overrideRefusal(err) === null) toast.error(errMessage(err, failure));
      throw err;
    }
  };

  return {
    // The draft the confirm planned, whichever it was: the card names no build here.
    promoteDraft: (override) => act("promote", null, "Couldn't promote the draft", override),
    publish: async () => {
      await publish.mutateAsync(appId);
    },
    rollbackTo: (build) => act("rollback", build.id, "Couldn't change the live build"),
    unpublish: async () => {
      await unpublish.mutateAsync(appId);
    },
    pending: publish.isPending || unpublish.isPending || move.isPending
  };
}

/**
 * The partner console's verbs —`/partners/{id}/apps/{appId}/publish`, gated by the
 * `manage_apps` ceiling server-side. No build history on this surface, so no rollback.
 */
export function usePartnerLifecycleActions(
  partnerId: string,
  orgId: string,
  appId: string
): LifecycleActions {
  const set = useSetAppPublished(partnerId);
  const run = async (published: boolean) => {
    try {
      await set.mutateAsync({ appId, orgId, published });
      toast.success(published ? "Published" : "Unpublished");
    } catch (err) {
      toast.error(errMessage(err, published ? "Couldn't publish" : "Couldn't unpublish"));
      throw err;
    }
  };
  return {
    publish: () => run(true),
    unpublish: () => run(false),
    pending: set.isPending
  };
}
