import { useState } from "react";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import type { AppLifecycle as Lifecycle } from "@/hooks/api/customApps/deriveLifecycle";
import { useAppLifecycle } from "@/hooks/api/customApps/useAppLifecycle";
import { useMissingSecretsForBuild } from "@/hooks/api/customApps/useAppSecrets";
import { useMovePlan } from "@/hooks/api/customApps/useProductionMove";
import type { AppAccessSummary } from "@/types/appAccess";
import type { MoveTarget } from "@/types/appPromote";
import type { CustomApp } from "@/types/apps";
import { lifecycleVerdict } from "../../appStatus";
import { MissingSecretsWarning } from "../AppDetail/components/MissingSecretsWarning";
import { useAdminLifecycleActions, usePartnerLifecycleActions } from "./actions";
import {
  LifecycleCard,
  type LifecyclePreflight,
  type LifecycleTarget,
  type LifecycleVariant
} from "./LifecycleCard";
import { movePreflight } from "./movePreflight";

export { LifecycleConfirm } from "./LifecycleConfirm";
export { LifecycleVerdictPill } from "./LifecycleVerdictPill";
export { LIFECYCLE_COPY } from "./lifecycleCopy";
export { PlannedMoveConfirm } from "./PlannedMoveConfirm";

/**
 * The card on a staff surface: the console, the dossier, the settings panel.
 * Reads through `useAppLifecycle` (shared cache with every other panel) and acts
 * through the admin `/customer-apps/{id}/*` mutations.
 *
 * Promote draft and roll back confirm on the move's dry-run plan — the one
 * `oxyc promote --dry-run` prints — and act on that plan: a blocker holds the verb,
 * as it refuses `oxyc promote`, and a schedule the plan retires is named in the
 * confirm and confirmed by it. A first publish still says only which `required`
 * secrets production lacks (`GET …/secrets?build=`), and waits for that answer.
 */
export const AppLifecycle = ({
  app,
  variant = "full",
  className
}: {
  app: CustomApp;
  variant?: LifecycleVariant;
  className?: string;
}) => {
  const lifecycle = useAppLifecycle(app);
  const [target, setTarget] = useState<LifecycleTarget | null>(null);
  const move: MoveTarget | null =
    target && target.act !== "publish" ? { act: target.act, buildId: target.buildId } : null;
  const plan = useMovePlan(app.id, move);
  const actions = useAdminLifecycleActions(
    app.id,
    move && plan.data ? { target: move, plan: plan.data } : undefined
  );
  // The warning's own query (same key, so one request): a first publish waits for it.
  const secretsCheck = useMissingSecretsForBuild(
    app.id,
    target?.act === "publish" ? target.buildId : undefined
  );

  if (lifecycle.isLoading) {
    return <Skeleton className='h-24 w-full rounded-lg' data-testid='app-lifecycle-loading' />;
  }
  if (lifecycle.error) {
    return (
      <p className='text-destructive text-xs' data-testid='app-lifecycle-error'>
        Couldn't load this app's builds.
      </p>
    );
  }
  return (
    <LifecycleCard
      lifecycle={lifecycle}
      actions={actions}
      orgSlug={app.org_slug}
      variant={variant}
      className={className}
      onTargetChange={setTarget}
      preflight={preflightFor(app.id, target, plan, secretsCheck.isLoading)}
    />
  );
};

/** What the open confirm says, and whether its verb waits. */
function preflightFor(
  appId: string,
  target: LifecycleTarget | null,
  plan: ReturnType<typeof useMovePlan>,
  secretsLoading: boolean
): LifecyclePreflight | undefined {
  if (!target) return undefined;
  if (target.act === "publish") {
    return {
      notice: <MissingSecretsWarning appId={appId} buildId={target.buildId} act='promote' />,
      hold: secretsLoading
    };
  }
  return movePreflight(plan);
}

/**
 * The same card on the partner console, where the backend offers publish /
 * unpublish and no build history. The lifecycle is therefore the publish fact alone:
 * no live build, no draft, and a verdict from `published` — the partner API has no
 * fleet-health or availability read, so nothing here claims one.
 */
export const PartnerAppLifecycle = ({
  app,
  partnerId,
  orgId,
  orgSlug,
  variant = "row",
  className
}: {
  app: Pick<AppAccessSummary, "id" | "published">;
  partnerId: string;
  orgId: string;
  orgSlug?: string | null;
  variant?: LifecycleVariant;
  className?: string;
}) => {
  const actions = usePartnerLifecycleActions(partnerId, orgId, app.id);
  return (
    <LifecycleCard
      lifecycle={partnerLifecycle(app.published)}
      actions={actions}
      orgSlug={orgSlug ?? null}
      variant={variant}
      className={className}
    />
  );
};

/** A lifecycle built from the one fact the partner API returns. */
function partnerLifecycle(published: boolean): Lifecycle {
  return {
    liveBuild: null,
    draftBuild: null,
    draftAhead: false,
    published,
    publishedAt: null,
    verdict: lifecycleVerdict({ published }),
    heldCount: null,
    heldTruncated: false,
    lastPromoted: { at: null, byEmail: null },
    builds: [],
    isLoading: false,
    error: null
  };
}
