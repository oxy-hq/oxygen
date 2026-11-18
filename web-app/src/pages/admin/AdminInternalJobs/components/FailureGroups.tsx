import { CheckCircle2 } from "lucide-react";
import { useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { useFailureGroups } from "@/hooks/api/internalJobs";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import { relativeTime } from "@/pages/admin/utils";
import type { FailureGroup } from "@/services/api/internalJobs";
import { groupQuery, groupsSentence, groupTone, spanPhrase } from "../failureGroups";
import { useLive } from "../LiveContext";

/** The window the groups cover. A week: a nightly failure reads as recurring. */
const DAYS = 7;

/**
 * What is broken, by cause — the page's first answer.
 *
 * The jobs list below is one row per job, newest first, so one workspace failing every
 * few hours filled it with copies of itself: on dev, 200 identical rows of one compile
 * failure out of 1,211, and whatever else broke that week below the fold. Here that is
 * one row — the error once, in full, with how many times and for how long — and every
 * other cause beside it.
 *
 * One grid container with subgrid rows (`pages/admin/CLAUDE.md`), so the columns line
 * up however long a workspace name or an error runs.
 */
export function FailureGroups() {
  const { paused } = useLive();
  const groups = useFailureGroups(DAYS, { paused });
  const [params, setParams] = useSearchParams();

  const showJobs = (g: FailureGroup) => {
    const update = new URLSearchParams(params);
    update.set("q", groupQuery(g));
    setParams(update, { replace: true });
    document.getElementById("admin-internal-jobs-console")?.scrollIntoView?.({ block: "start" });
  };

  return (
    <section className='space-y-2' data-testid='admin-internal-jobs-failing'>
      <h3 className='font-semibold text-sm'>Failing now</h3>
      <AdminAsync
        query={groups}
        noun='failure groups'
        skeleton={<Skeleton className='h-24 w-full' />}
      >
        {(data) => (
          <>
            <p
              className='text-muted-foreground text-xs'
              data-testid='admin-internal-jobs-failing-sentence'
            >
              {groupsSentence(data.groups, data.days)}
            </p>
            {data.groups.length === 0 ? (
              <p className='flex items-center gap-2 text-muted-foreground text-xs'>
                <CheckCircle2 className={cn("size-3.5", ADMIN_TONE.ok.text)} aria-hidden />
                No job has failed or died this week.
              </p>
            ) : (
              <ul
                className='grid grid-cols-[minmax(7rem,auto)_minmax(9rem,13rem)_minmax(0,1fr)_auto_auto] overflow-hidden rounded-lg border border-border/60 bg-card'
                data-testid='admin-internal-jobs-groups'
              >
                {data.groups.map((g) => (
                  <GroupRow
                    key={`${g.task_type}:${g.workspace_id}:${g.signature}`}
                    g={g}
                    onShowJobs={() => showJobs(g)}
                  />
                ))}
              </ul>
            )}
          </>
        )}
      </AdminAsync>
    </section>
  );
}

function GroupRow({ g, onShowJobs }: { g: FailureGroup; onShowJobs: () => void }) {
  const tone = ADMIN_TONE[groupTone(g)];
  const [open, setOpen] = useState(false);
  return (
    <li
      className='col-span-full grid grid-cols-subgrid items-start gap-x-4 border-border/50 border-b px-3 py-2.5 text-xs last:border-b-0'
      data-testid='admin-internal-jobs-group'
      data-group-type={g.task_type}
    >
      <span className='flex items-center gap-2 font-mono text-[11px]'>
        <span className={cn("size-2 shrink-0 rounded-full", tone.dot)} aria-hidden />
        {g.task_type}
      </span>
      <span className='min-w-0'>
        <span className='block truncate font-medium'>{g.workspace_name ?? "No workspace"}</span>
        <span className='block truncate text-muted-foreground'>{g.org_name ?? "—"}</span>
      </span>
      {/* The error is the most important text on this page, so it gets its own
          column. It shows its first two lines, where a failure says what it is, and
          prints in full on a click: a page of raw JSON errors in full was a wall to
          read past to the next group. */}
      <button
        type='button'
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        title={open ? undefined : "Show the whole error"}
        className={cn(
          "min-w-0 break-words text-left font-mono text-[11px] text-foreground/90 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
          open ? "whitespace-pre-wrap" : "line-clamp-2 hover:text-foreground"
        )}
        data-testid='admin-internal-jobs-group-error'
      >
        {g.latest_error ?? g.signature ?? "No error recorded on the run."}
      </button>
      <span className='text-right tabular-nums'>
        <span className={cn("block font-semibold", tone.text)}>
          {g.occurrences.toLocaleString("en-US")}×
        </span>
        {g.dead > 0 ? (
          <span className={cn("block text-[10px]", ADMIN_TONE.danger.text)}>{g.dead} dead</span>
        ) : null}
      </span>
      <span className='space-y-1 text-right'>
        <span className='block whitespace-nowrap text-muted-foreground'>
          {spanPhrase(g)}, last {relativeTime(g.last_seen)}
        </span>
        <span className='flex justify-end gap-3 whitespace-nowrap'>
          <button
            type='button'
            onClick={onShowJobs}
            className='font-medium underline-offset-2 hover:underline'
            data-testid='admin-internal-jobs-group-show'
          >
            Show jobs
          </button>
          {g.task_type === "compile" && g.workspace_id ? (
            <Link
              to={`${ROUTES.ADMIN.COMPILES}?q=${encodeURIComponent(g.workspace_id)}`}
              className='font-medium underline-offset-2 hover:underline'
              data-testid='admin-internal-jobs-group-compiles'
            >
              Compile history
            </Link>
          ) : null}
        </span>
      </span>
    </li>
  );
}
