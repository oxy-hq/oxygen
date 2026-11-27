import { ChevronRight, Eye } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { useMarkChangesSeen } from "@/hooks/api/customApps/useBuildChecks";
import { cn } from "@/libs/shadcn/utils";
import { timeAgo } from "@/libs/utils/date";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { BuildCheck, BuildCheckDetail, BuildCheckSeen } from "@/types/appBuildChecks";
import { shortBuildId } from "../../../AppLifecycle/lifecycleCopy";
import type { ChecksScope } from "../checksScope";
import { changedScreens, changesCount, seenOf, seenRefusal } from "../screensModel";
import { ChangedScreen } from "./ChangedScreen";

/**
 * What the build changes on screen against production: how many screens differ, then
 * each one that does as a pair of pictures, then the ones that do not behind one line.
 * A person marks the changed screens seen here, and once someone has it says who.
 */
export const ChangesEvidence = ({
  scope,
  check,
  detail
}: {
  scope: ChecksScope;
  check: Pick<BuildCheck, "attempt">;
  detail: BuildCheckDetail;
}) => {
  const screens = changedScreens(detail);
  const { changed, compared } = changesCount(detail, screens);
  const [showSame, setShowSame] = useState(false);
  if (compared === 0 && screens.length === 0) return null;
  const differing = screens.filter((s) => s.changed);
  const same = screens.filter((s) => !s.changed);
  return (
    <div className='flex flex-col gap-1' data-testid='build-check-changes'>
      <div className='flex flex-wrap items-center justify-between gap-x-3 gap-y-1 text-xs'>
        <span data-testid='build-check-changes-count'>
          <span className='font-medium'>
            {changed} of {compared}
          </span>{" "}
          differ
          {detail.production_build_id && (
            <span className='text-muted-foreground'>
              {" "}
              · production{" "}
              <span className='font-mono' title={detail.production_build_id}>
                {shortBuildId(detail.production_build_id)}
              </span>
            </span>
          )}
        </span>
        <Seen scope={scope} attempt={check.attempt} seen={seenOf(detail)} changed={changed} />
      </div>
      {differing.length > 0 && (
        <ul className='flex flex-col' data-testid='build-check-changes-differing'>
          {differing.map((screen) => (
            <ChangedScreen key={screen.key} scope={scope} attempt={check.attempt} screen={screen} />
          ))}
        </ul>
      )}
      {same.length > 0 && (
        <>
          <button
            type='button'
            aria-expanded={showSame}
            onClick={() => setShowSame((was) => !was)}
            className='flex items-center gap-2 py-1 text-left text-muted-foreground text-xs hover:bg-muted/40'
            data-testid='build-check-changes-same-toggle'
          >
            <ChevronRight
              className={cn("size-3 shrink-0 transition-transform", showSame && "rotate-90")}
              aria-hidden
            />
            {same.length} the same
          </button>
          {showSame && (
            <ul className='flex flex-col' data-testid='build-check-changes-same'>
              {same.map((screen) => (
                <ChangedScreen
                  key={screen.key}
                  scope={scope}
                  attempt={check.attempt}
                  screen={screen}
                />
              ))}
            </ul>
          )}
        </>
      )}
    </div>
  );
};

/** Who saw the changed screens, or the button that says "I have". The button shows
 *  whenever a screen changed and nobody has, on staging's attempt; it is the primary
 *  one while the app holds Promote on it. */
const Seen = ({
  scope,
  attempt,
  seen,
  changed
}: {
  scope: ChecksScope;
  attempt: number;
  seen: BuildCheckSeen | null;
  changed: number;
}) => {
  const mark = useMarkChangesSeen(scope.appId);
  if (seen) {
    return (
      <span
        className={cn("inline-flex min-w-0 items-center gap-1", ADMIN_TONE.ok.text)}
        title={seen.at ? new Date(seen.at).toLocaleString() : undefined}
        data-testid='build-check-seen'
      >
        <Eye className='size-3.5 shrink-0' aria-hidden />
        <span className='truncate'>
          Seen by {seen.by}
          {seen.at ? ` · ${timeAgo(seen.at)}` : ""}
        </span>
      </span>
    );
  }
  if (changed === 0 || scope.environment !== "staging") return null;
  return (
    <span className='flex items-center gap-2'>
      {mark.isError && (
        <span className={ADMIN_TONE.danger.text} data-testid='build-check-seen-refused'>
          {seenRefusal(mark.error)}
        </span>
      )}
      <Button
        size='sm'
        variant={scope.seenRequired ? "default" : "outline"}
        className='h-6 gap-1 px-2 text-xs'
        disabled={mark.isPending}
        onClick={() => mark.mutate({ build: scope.build, attempt })}
        data-testid='build-check-mark-seen'
      >
        <Eye className='size-3' aria-hidden />
        Mark as seen
      </Button>
    </span>
  );
};
