import { ChevronDown, History } from "lucide-react";
import type { ReactNode } from "react";
import { Link } from "react-router-dom";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Button } from "@/components/ui/shadcn/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger
} from "@/components/ui/shadcn/dropdown-menu";
import { AdminStatusPill } from "@/pages/admin/components/AdminStatusPill";
import type { AppBuild } from "@/types/apps";
import type { SmokeLine } from "../AppConsole/BuildChecks/promoteGate";
import { relativeTime } from "../AppDetail/components/Activity/relativeTime";
import type { LifecycleSmoke } from "./LifecycleCard";
import { shortBuildId } from "./lifecycleCopy";

/** The lifecycle card's own pieces: a build's line, the rollback menu, the smoke chip,
 *  and the Smoke and Checks rows of its list. */

/** `0123456 · 3h ago · who@x`, with the promotion line when the build is the live one. */
export const BuildLine = ({
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
export const RollbackMenu = ({
  targets,
  disabled,
  primary = false,
  onPick
}: {
  targets: AppBuild[];
  disabled: boolean;
  /** Going back is the first thing to do (production's smoke failed). */
  primary?: boolean;
  onPick: (b: AppBuild) => void;
}) => (
  <DropdownMenu>
    <DropdownMenuTrigger asChild>
      <Button
        size='sm'
        variant={primary ? "default" : "outline"}
        className='h-7 gap-1.5 text-xs'
        disabled={disabled}
        data-testid='app-lifecycle-rollback'
      >
        <History className='size-3.5' />
        Roll back to…
        <ChevronDown className='size-3' />
      </Button>
    </DropdownMenuTrigger>
    <DropdownMenuContent align='end' className={`${DESIGN_REFERENCE} max-h-80 overflow-auto`}>
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

/** Production's smoke in one chip, linking to its evidence when the surface has it. */
export const SmokePill = ({ line, to }: { line: SmokeLine; to?: string }) => {
  const pill = <AdminStatusPill tone={line.tone} label={line.label} />;
  return to ? (
    <Link
      to={to}
      className='inline-flex rounded-full hover:underline'
      data-testid='app-lifecycle-smoke-link'
    >
      {pill}
    </Link>
  ) : (
    pill
  );
};

/** The card's Smoke row: how production's own smoke of the live build went. Nothing
 *  on a surface that reads no smoke. */
export const SmokeRow = ({ smoke }: { smoke: LifecycleSmoke | undefined }) =>
  smoke && (
    <>
      <dt className='text-muted-foreground'>Smoke</dt>
      <dd
        className='min-w-0'
        data-testid='app-lifecycle-smoke'
        data-state={smoke.line.state}
        title={smoke.line.title ?? undefined}
      >
        <SmokePill line={smoke.line} to={smoke.to} />
      </dd>
    </>
  );

/** The card's Checks row: the staging build's checks, as the surface says them.
 *  Nothing on a surface that says none. */
export const ChecksRow = ({ checks }: { checks: ReactNode }) =>
  checks && (
    <>
      <dt className='text-muted-foreground'>Checks</dt>
      <dd className='min-w-0' data-testid='app-lifecycle-checks'>
        {checks}
      </dd>
    </>
  );
