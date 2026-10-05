import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/shadcn/popover";
import {
  isNotFound,
  STAGING_HELD_LIMIT,
  useStagingHeld
} from "@/hooks/api/customApps/useCustomApps";
import type { StagingHeldEntry, StagingHeldWrite } from "@/services/api/customApps";
import { relativeTime } from "./components/Activity/relativeTime";

/**
 * What staging held, for the developer who caused it.
 *
 * Mounted by `LivePreview` only while the console is actually framing
 * staging — channel `draft` *and* the target resolved to the staging host
 * (`draftTarget.kind === "staging"`). Never over the production/Live frame,
 * never while the target is pending or unavailable: see the ledger ruling in
 * the staging-console plan. Because mounting already encodes that state,
 * this component always polls (`useStagingHeld(appId, true)`); there is no
 * separate on/off prop to keep in sync with the caller's condition.
 *
 * A 404 from the list means the caller may not open this app's staging
 * (`custom_apps_staging_held::list_held` — same answer as an app that does
 * not exist). That replaces the banner outright rather than showing a count
 * of zero, which would claim standing the caller doesn't have.
 *
 * The count is held *writes* — a row is one held call and may carry several
 * — matching the popover's one line per write. The list is capped at
 * `STAGING_HELD_LIMIT` rows; a full list may be truncated, so the count
 * then reads "N+".
 */
export const StagingBanner = ({ appId }: { appId: string }) => {
  const { data, error, isLoading } = useStagingHeld(appId, true);

  if (isNotFound(error)) {
    return (
      <p
        data-testid='admin-app-staging-banner-forbidden'
        className='shrink-0 border-b bg-muted/30 px-3 py-1.5 text-muted-foreground text-xs'
      >
        You can't open this app's staging
      </p>
    );
  }

  // Any other error, or still loading: say nothing rather than guess a count.
  // The banner is a bonus read-out on top of the frame, not something the
  // frame's availability depends on.
  if (isLoading || !data) return null;

  const n = data.reduce((sum, entry) => sum + entry.writes.length, 0);
  const truncated = data.length >= STAGING_HELD_LIMIT;

  return (
    <div
      data-testid='admin-app-staging-banner'
      className='flex shrink-0 items-center gap-1 border-b bg-muted/30 px-3 py-1.5 text-xs'
    >
      <span className='text-muted-foreground'>
        Staging · live reads · writes held or isolated ·{" "}
      </span>
      <Popover>
        <PopoverTrigger
          data-testid='admin-app-staging-banner-count'
          className='font-medium text-foreground underline decoration-dotted underline-offset-2 hover:text-primary'
        >
          {truncated ? `${n}+` : n} {n === 1 && !truncated ? "write" : "writes"} held
        </PopoverTrigger>
        <PopoverContent
          data-testid='admin-app-staging-held-list'
          align='start'
          className='w-96 p-0'
        >
          <HeldList entries={data} />
        </PopoverContent>
      </Popover>
    </div>
  );
};

const HeldList = ({ entries }: { entries: StagingHeldEntry[] }) => {
  if (entries.length === 0) {
    return (
      <p
        data-testid='admin-app-staging-held-empty'
        className='p-4 text-center text-muted-foreground text-xs leading-relaxed'
      >
        Nothing held yet — writes staging can't isolate appear here.
      </p>
    );
  }

  // One row per write, not per call: a single staging invocation can hold
  // several writes, and each gets its own `plane verb namespace.table` line
  // under the shared time + function it happened in.
  return (
    <ul className='max-h-80 divide-y overflow-auto text-xs'>
      {entries.flatMap((entry, i) =>
        entry.writes.map((write, j) => (
          // biome-ignore lint/suspicious/noArrayIndexKey: held rows have no stable id
          <li key={`${i}-${j}`} data-testid='admin-app-staging-held-row' className='px-3 py-2'>
            <span className='text-muted-foreground'>
              {relativeTime(entry.at)} · {entry.function} ·{" "}
            </span>
            <span className='font-mono'>
              {write.plane} {write.verb} {qualifiedName(write)}
            </span>
          </li>
        ))
      )}
    </ul>
  );
};

/** `namespace.table`, with no stray dot when either part is empty. */
export const qualifiedName = ({
  namespace,
  table
}: Pick<StagingHeldWrite, "namespace" | "table">) =>
  [namespace, table].filter((part) => part).join(".");
