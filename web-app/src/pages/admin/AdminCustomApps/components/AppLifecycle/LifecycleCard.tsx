import { EyeOff, Send } from "lucide-react";
import type { ReactNode } from "react";
import { Button } from "@/components/ui/shadcn/button";
import type { AppLifecycle } from "@/hooks/api/customApps/deriveLifecycle";
import { cn } from "@/libs/shadcn/utils";
import type { AppBuild } from "@/types/apps";
import type { SmokeLine } from "../AppConsole/BuildChecks/promoteGate";
import type { LifecycleActions } from "./actions";
import { LifecycleCardConfirm } from "./LifecycleCardConfirm";
import { BuildLine, ChecksRow, RollbackMenu, SmokeRow } from "./LifecycleCardParts";
import { PromoteButton, RollbackBeforeButton } from "./LifecycleCardVerbs";
import { LifecycleVerdictPill } from "./LifecycleVerdictPill";
import { LIFECYCLE_COPY } from "./lifecycleCopy";
import type { OverrideOffer } from "./planChecks";
import { type ConfirmedRun, useLifecycleConfirm } from "./useLifecycleConfirm";

/**
 * What a surface asks the card to show. `full` is the console and the dossier: the
 * live build, the draft-ahead line, held writes, and every verb. `row` is a table
 * cell — the partner fleet, an org's app list — verdict plus the verbs, one line.
 */
export type LifecycleVariant = "full" | "row";

/** The build an open confirm would put live, and by which verb. */
export interface LifecycleTarget {
  /** `app_builds.id`. */
  buildId: string;
  act: "promote" | "rollback" | "publish";
}

/** What a surface checked about the target, said inside the confirm. `hold` keeps
 *  the verb disabled until the check has answered. `override` is the offer to promote
 *  past the build's checks with a written reason, when the plan makes one. */
export interface LifecyclePreflight {
  notice: ReactNode;
  hold: boolean;
  override?: OverrideOffer;
}

/** Production's own smoke after a move, as a surface that reads it says it. */
export interface LifecycleSmoke {
  line: SmokeLine;
  /** Where its evidence is; absent, the chip is not a link. */
  to?: string;
  /** The build production served before, when the smoke failed and a rollback can
   *  still choose it. */
  rollback?: AppBuild | null;
}

/**
 * What a failed production smoke changes about the verbs: going back is the first
 * thing to offer, to the build production served before (`before`) when that is
 * known, else through the menu of every retained build (`menuFirst`).
 */
function rollbackAfterSmoke(smoke: LifecycleSmoke | undefined, canRollback: boolean | undefined) {
  const smokeFailed = smoke?.line.state === "failed" && !!canRollback;
  const before = smokeFailed ? (smoke?.rollback ?? null) : null;
  return { smokeFailed, before, menuFirst: smokeFailed && !before };
}

/**
 * The lifecycle card — one component that answers "what is live, is it OK, and how do
 * I promote or roll back" wherever an app appears.
 *
 * Before this the same three questions were spread across four places with three
 * meanings: a Published/Draft card in Settings (publish / unpublish), "Make live" in the
 * build history (a pointer move to any build), a Switch on each partner row, and the
 * preview's channel toggle — which merely chose what the iframe framed, in the same
 * words. The card owns the verbs; the preview's switch now says Production / Staging.
 *
 * Presentational: data and verbs come in, the confirm is the only state it holds. The
 * admin and partner wrappers in `index.tsx` are what connect it to a backend.
 */
export const LifecycleCard = ({
  lifecycle,
  actions,
  orgSlug,
  variant = "full",
  className,
  onTargetChange,
  preflight,
  checks,
  promoteWaits,
  smoke
}: {
  lifecycle: AppLifecycle;
  actions: LifecycleActions;
  /** For the publish confirm's "Members of <org> can open it"; `null` when unknown. */
  orgSlug: string | null;
  variant?: LifecycleVariant;
  className?: string;
  /** Told which build the open confirm would put live, and `null` when it closes —
   *  so a surface that knows the app can check that build (the admin console's
   *  missing-secrets warning) and hand the answer back as `preflight`. */
  onTargetChange?: (target: LifecycleTarget | null) => void;
  preflight?: LifecyclePreflight;
  /** The staging build's checks, as a surface that reads them says it (the admin
   *  console's Checks row). Absent, the card has no Checks row. */
  checks?: ReactNode;
  /** A check the app requires is still open on the staging build: Promote waits. */
  promoteWaits?: boolean;
  /** Absent, the card has no Smoke row. */
  smoke?: LifecycleSmoke;
}) => {
  // The card's state: what is about to happen, and why a promote past the checks was
  // refused.
  const confirming = useLifecycleConfirm(onTargetChange);
  const { ask } = confirming;
  const { liveBuild, draftBuild, draftAhead, published, verdict, heldCount, builds } = lifecycle;
  const liveId = liveBuild?.build_id ?? null;

  const askRollback = (b: AppBuild) =>
    ask(
      LIFECYCLE_COPY.rollback(b.build_id, liveId),
      () => (actions.rollbackTo as (b: AppBuild) => Promise<void>)(b),
      { buildId: b.id, act: "rollback" }
    );

  // Which verbs apply, from the lifecycle alone. A verb the surface did not provide
  // (no `actions.rollbackTo` on the partner console) renders nothing.
  const canPromote = published && draftAhead && draftBuild && actions.promoteDraft;
  // The partner console knows no builds, so it may publish on the publish fact alone;
  // the admin console, which does, only offers it when there is a draft to point at.
  const canPublish = !published && actions.publish && (builds.length === 0 || draftBuild);
  const rollbackTargets = builds.filter((b) => !b.is_published);
  const canRollback = published && actions.rollbackTo && rollbackTargets.length > 0;
  const canUnpublish = published && actions.unpublish;
  const { smokeFailed, before, menuFirst } = rollbackAfterSmoke(smoke, canRollback);

  const controls = (
    <div className='flex flex-wrap items-center gap-1.5' data-testid='app-lifecycle-actions'>
      {before && (
        <RollbackBeforeButton build={before} disabled={actions.pending} onPick={askRollback} />
      )}
      {canPromote && draftBuild && (
        <PromoteButton
          waits={promoteWaits}
          secondary={smokeFailed}
          disabled={actions.pending}
          onClick={() =>
            ask(
              LIFECYCLE_COPY.promote(draftBuild.build_id, liveId),
              actions.promoteDraft as ConfirmedRun,
              { buildId: draftBuild.id, act: "promote" }
            )
          }
        />
      )}
      {canPublish && (
        <Button
          size='sm'
          className='h-7 gap-1.5 text-xs'
          disabled={actions.pending}
          onClick={() =>
            ask(
              LIFECYCLE_COPY.publish(orgSlug),
              actions.publish as () => Promise<void>,
              // A first publish with a draft makes that draft live.
              draftBuild ? { buildId: draftBuild.id, act: "publish" } : undefined
            )
          }
          data-testid='app-lifecycle-publish'
        >
          <Send className='size-3.5' />
          Publish
        </Button>
      )}
      {canRollback && (
        <RollbackMenu
          targets={rollbackTargets}
          disabled={actions.pending}
          primary={menuFirst}
          onPick={askRollback}
        />
      )}
      {canUnpublish && (
        <Button
          size='sm'
          variant='ghost'
          className='h-7 gap-1.5 text-muted-foreground text-xs'
          disabled={actions.pending}
          onClick={() => ask(LIFECYCLE_COPY.unpublish(), actions.unpublish as () => Promise<void>)}
          data-testid='app-lifecycle-unpublish'
        >
          <EyeOff className='size-3.5' />
          Unpublish
        </Button>
      )}
    </div>
  );

  const dialog = (
    <LifecycleCardConfirm confirming={confirming} busy={actions.pending} preflight={preflight} />
  );

  if (variant === "row") {
    return (
      <div
        className={cn("flex flex-wrap items-center justify-between gap-2", className)}
        data-testid='app-lifecycle'
        data-variant='row'
      >
        <LifecycleVerdictPill verdict={verdict} compact data-testid='app-lifecycle-verdict' />
        {controls}
        {dialog}
      </div>
    );
  }

  return (
    <div
      className={cn("flex flex-col gap-3 border border-border/60 bg-card p-3", className)}
      data-testid='app-lifecycle'
      data-variant='full'
    >
      <div className='flex flex-wrap items-center justify-between gap-2'>
        <LifecycleVerdictPill verdict={verdict} data-testid='app-lifecycle-verdict' />
        {controls}
      </div>

      <dl className='grid grid-cols-[max-content_minmax(0,1fr)] gap-x-3 gap-y-1 text-xs'>
        {/* "Production" and "Staging", the names the preview toolbar and the publish
            pipeline use (`published_to: staging`). This card said "Live" and "Draft"
            for the same two places, so one app was described in two vocabularies a
            panel apart. */}
        <dt className='text-muted-foreground'>Production</dt>
        <dd className='min-w-0 truncate' data-testid='app-lifecycle-live'>
          {liveBuild ? (
            <BuildLine build={liveBuild} promoted={lifecycle.lastPromoted} />
          ) : published ? (
            <span className='text-muted-foreground'>Published, no build recorded</span>
          ) : (
            <span className='text-muted-foreground'>Nothing in production — staging only</span>
          )}
        </dd>

        <SmokeRow smoke={smoke} />

        <dt className='text-muted-foreground'>Staging</dt>
        <dd className='min-w-0 truncate' data-testid='app-lifecycle-draft'>
          {draftAhead && draftBuild ? (
            <span className='flex items-center gap-1.5'>
              <span className='size-1.5 shrink-0 rounded-full bg-warning' aria-hidden />
              <BuildLine build={draftBuild} />
              <span className='text-muted-foreground'>· ahead of production</span>
            </span>
          ) : (
            <span className='text-muted-foreground'>
              {draftBuild ? "Same as production" : "Nothing on staging"}
            </span>
          )}
        </dd>

        <ChecksRow checks={checks} />

        {heldCount !== null && (
          <>
            <dt className='text-muted-foreground'>Held</dt>
            <dd data-testid='app-lifecycle-held'>
              {lifecycle.heldTruncated ? `${heldCount}+` : heldCount}{" "}
              {heldCount === 1 && !lifecycle.heldTruncated ? "write" : "writes"} held on staging
            </dd>
          </>
        )}
      </dl>
      {dialog}
    </div>
  );
};
