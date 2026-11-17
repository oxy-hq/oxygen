import { ArrowUpCircle, ChevronDown, EyeOff, History, Send } from "lucide-react";
import { type ReactNode, useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger
} from "@/components/ui/shadcn/dropdown-menu";
import type { AppLifecycle } from "@/hooks/api/customApps/deriveLifecycle";
import { cn } from "@/libs/shadcn/utils";
import type { AppBuild } from "@/types/apps";
import { relativeTime } from "../AppDetail/components/Activity/relativeTime";
import type { LifecycleActions } from "./actions";
import { LifecycleConfirm } from "./LifecycleConfirm";
import { LifecycleVerdictPill } from "./LifecycleVerdictPill";
import { type ConfirmCopy, LIFECYCLE_COPY, shortBuildId } from "./lifecycleCopy";

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
 *  the verb disabled until the check has answered. */
export interface LifecyclePreflight {
  notice: ReactNode;
  hold: boolean;
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
  preflight
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
}) => {
  // One piece of state: what is about to happen. `null` is "nothing".
  const [pending, setPending] = useState<{ copy: ConfirmCopy; run: () => Promise<void> } | null>(
    null
  );
  const { liveBuild, draftBuild, draftAhead, published, verdict, heldCount, builds } = lifecycle;
  const liveId = liveBuild?.build_id ?? null;

  const ask = (copy: ConfirmCopy, run: () => Promise<void>, target?: LifecycleTarget) => {
    setPending({ copy, run });
    onTargetChange?.(target ?? null);
  };
  const close = () => {
    setPending(null);
    onTargetChange?.(null);
  };
  const confirm = async () => {
    if (!pending) return;
    try {
      await pending.run();
      close();
    } catch {
      // The verb toasted its own failure; the dialog stays so the operator can retry.
    }
  };

  // Which verbs apply, from the lifecycle alone. A verb the surface did not provide
  // (no `actions.rollbackTo` on the partner console) renders nothing.
  const canPromote = published && draftAhead && draftBuild && actions.promoteDraft;
  // The partner console knows no builds, so it may publish on the publish fact alone;
  // the admin console, which does, only offers it when there is a draft to point at.
  const canPublish = !published && actions.publish && (builds.length === 0 || draftBuild);
  const rollbackTargets = builds.filter((b) => !b.is_published);
  const canRollback = published && actions.rollbackTo && rollbackTargets.length > 0;
  const canUnpublish = published && actions.unpublish;

  const controls = (
    <div className='flex flex-wrap items-center gap-1.5' data-testid='app-lifecycle-actions'>
      {canPromote && draftBuild && (
        <Button
          size='sm'
          className='h-7 gap-1.5 text-xs'
          disabled={actions.pending}
          onClick={() =>
            ask(
              LIFECYCLE_COPY.promote(draftBuild.build_id, liveId),
              actions.promoteDraft as () => Promise<void>,
              { buildId: draftBuild.id, act: "promote" }
            )
          }
          data-testid='app-lifecycle-promote'
        >
          <ArrowUpCircle className='size-3.5' />
          Promote to production
        </Button>
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
          onPick={(b) =>
            ask(
              LIFECYCLE_COPY.rollback(b.build_id, liveId),
              () => (actions.rollbackTo as (b: AppBuild) => Promise<void>)(b),
              { buildId: b.id, act: "rollback" }
            )
          }
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
    <LifecycleConfirm
      copy={pending?.copy ?? null}
      pending={actions.pending}
      onConfirm={confirm}
      onCancel={close}
      notice={pending ? preflight?.notice : undefined}
      hold={!!pending && !!preflight?.hold}
    />
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
      className={cn(
        "flex flex-col gap-3 rounded-lg border border-border/60 bg-card p-3",
        className
      )}
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

/** `0123456 · 3h ago · who@x`, with the promotion line when the build is the live one. */
const BuildLine = ({
  build,
  promoted
}: {
  build: AppBuild;
  promoted?: { at: string | null; byEmail: string | null };
}) => (
  <span className='inline-flex min-w-0 flex-wrap items-baseline gap-x-1.5'>
    <span className='font-mono' title={build.build_id}>
      {shortBuildId(build.build_id)}
    </span>
    <span className='text-muted-foreground' title={new Date(build.created_at).toLocaleString()}>
      built {relativeTime(build.created_at)}
      {build.published_by_email ? ` by ${build.published_by_email}` : ""}
    </span>
    {promoted?.at && (
      <span className='text-muted-foreground' title={new Date(promoted.at).toLocaleString()}>
        · promoted {relativeTime(promoted.at)}
        {promoted.byEmail ? ` by ${promoted.byEmail}` : ""}
      </span>
    )}
  </span>
);

/** "Roll back to…" — every retained build that is not live, newest first. */
const RollbackMenu = ({
  targets,
  disabled,
  onPick
}: {
  targets: AppBuild[];
  disabled: boolean;
  onPick: (b: AppBuild) => void;
}) => (
  <DropdownMenu>
    <DropdownMenuTrigger asChild>
      <Button
        size='sm'
        variant='outline'
        className='h-7 gap-1.5 text-xs'
        disabled={disabled}
        data-testid='app-lifecycle-rollback'
      >
        <History className='size-3.5' />
        Roll back to…
        <ChevronDown className='size-3' />
      </Button>
    </DropdownMenuTrigger>
    <DropdownMenuContent align='end' className='max-h-80 overflow-auto'>
      {targets.map((b) => (
        <DropdownMenuItem
          key={b.id}
          onSelect={() => onPick(b)}
          className='flex items-baseline gap-2 text-xs'
          data-testid={`app-lifecycle-rollback-${shortBuildId(b.build_id)}`}
        >
          <span className='font-mono'>{shortBuildId(b.build_id)}</span>
          <span className='text-muted-foreground'>
            {relativeTime(b.created_at)}
            {b.is_draft ? " · draft" : ""}
          </span>
        </DropdownMenuItem>
      ))}
    </DropdownMenuContent>
  </DropdownMenu>
);
