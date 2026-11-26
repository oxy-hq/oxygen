import { History } from "lucide-react";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { Release, ReleaseTimeline } from "@/types/appReleases";
import { relativeTime } from "../../Activity/relativeTime";
import { IncompleteSource, SourceLink } from "../../BuildHistory/SourceLink";
import { reasonText, releaseLabel, STARTED_BY_LABEL, VERDICT_LABEL, verdictTone } from "../lanes";
import { ReleaseMetrics } from "./ReleaseMetrics";

/**
 * One production release: which build, how it went live, what it measured beside
 * the release before it, the migrations it ran and the failures first seen on it.
 * A red release serving now carries its Roll back — to the release it was compared
 * with, through the move's dry-run plan.
 */
export const ReleaseRow = ({
  release: r,
  previous,
  parts,
  onOpenFunction,
  onRollback
}: {
  release: Release;
  previous: Release | null;
  parts: ReleaseTimeline["parts"];
  onOpenFunction: (name: string) => void;
  onRollback: (release: Release) => void;
}) => {
  const tone = ADMIN_TONE[verdictTone(r.verdict.kind)];
  const b = r.build;
  return (
    <li
      className='flex flex-col gap-2 border border-border bg-card px-3 py-2'
      data-testid={`admin-app-release-${r.index}`}
    >
      <div className='flex flex-wrap items-center gap-x-2 gap-y-1 text-xs'>
        <span className='font-mono' title={b?.build_id}>
          {b ? releaseLabel(b.build_id) : r.unpublished ? "Unpublished" : "Pruned build"}
        </span>
        {r.current && <Badge className='h-5'>Live</Badge>}
        <span className='text-muted-foreground'>
          {STARTED_BY_LABEL[r.started_by.action]} {relativeTime(r.started_at)}
          {r.started_by.actor ? ` · ${r.started_by.actor}` : ""}
          {r.clipped ? " · counted from the window's start" : ""}
        </span>
        {b &&
          (b.source_repo?.trim() && b.commit_sha?.trim() ? (
            <SourceLink repo={b.source_repo} sha={b.commit_sha} branch={b.source_branch} />
          ) : (
            <IncompleteSource repo={b.source_repo} sha={b.commit_sha} />
          ))}
      </div>
      <div
        className={cn("flex flex-col gap-0.5 px-2 py-1 ring-1 ring-inset", tone.bg, tone.ring)}
        data-testid={`admin-app-release-${r.index}-verdict`}
        data-verdict={r.verdict.kind}
      >
        <span className={cn("font-medium text-xs", tone.text)}>
          {VERDICT_LABEL[r.verdict.kind]}
          {previous?.build ? ` · vs ${releaseLabel(previous.build.build_id)}` : ""}
        </span>
        {r.verdict.reasons.map((reason) => (
          <span
            key={`${reason.metric}-${reason.screen ?? ""}`}
            className='text-muted-foreground text-xs'
          >
            {reasonText(reason)} <span className='text-[10px]'>({reason.rule})</span>
          </span>
        ))}
        {r.verdict.why && <span className='text-muted-foreground text-xs'>{r.verdict.why}</span>}
      </div>
      <ReleaseMetrics release={r} previous={previous} parts={parts} />
      {r.migrations.length > 0 && (
        <p className='text-xs' data-testid={`admin-app-release-${r.index}-migrations`}>
          Ran {r.migrations.length} migration{r.migrations.length === 1 ? "" : "s"}:{" "}
          <span className='font-mono'>
            {r.migrations.map((m) => `${m.store}/${m.filename}`).join(", ")}
          </span>
        </p>
      )}
      {r.new_issues.length > 0 && (
        <div className='flex flex-wrap items-center gap-1 text-xs'>
          <span className='text-muted-foreground'>First seen here:</span>
          {r.new_issues.map((i) => (
            <Button
              key={`${i.function_name}:${i.fingerprint}`}
              variant='outline'
              size='sm'
              className='h-6 px-2 font-mono text-xs'
              onClick={() => onOpenFunction(i.function_name)}
              data-testid={`admin-app-release-${r.index}-issue-${i.fingerprint}`}
            >
              {i.function_name} · {i.occurrences}×
            </Button>
          ))}
        </div>
      )}
      {r.rollback?.offered && r.rollback.to && (
        <Button
          size='sm'
          variant='destructive'
          className='h-7 w-fit gap-1.5 text-xs'
          onClick={() => onRollback(r)}
          data-testid={`admin-app-release-${r.index}-rollback`}
        >
          <History className='size-3.5' />
          Roll back to {releaseLabel(r.rollback.to.build_id)}
        </Button>
      )}
    </li>
  );
};
