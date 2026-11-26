import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { ReleaseLaneEvent, ReleaseTimeline } from "@/types/appReleases";
import { relativeTime } from "../../Activity/relativeTime";
import { LANE_LABEL, position, releaseLabel, verdictTone } from "../lanes";

/**
 * Production, staging and sandboxes over one time axis. Production also draws its
 * releases as segments tinted by verdict, so a red stretch reads at a glance; every
 * pointer move is a mark on its lane. Without non-production reach the two lower
 * lanes are replaced by one line saying so, never drawn empty.
 */
export const ReleaseLanes = ({ data }: { data: ReleaseTimeline }) => {
  const { since, until } = data.window;
  const at = (iso: string) => position(iso, since, until);
  const hidden = data.non_production === "not_permitted";
  return (
    <div className='flex flex-col gap-1' data-testid='admin-app-releases-lanes'>
      {data.lanes
        .filter((lane) => !hidden || lane.lane === "production")
        .map((lane) => (
          <div
            key={lane.lane}
            className='flex items-center gap-2'
            data-testid={`admin-app-releases-lane-${lane.lane}`}
          >
            <span className='w-18 shrink-0 text-[10px] text-muted-foreground uppercase tracking-[0.16em]'>
              {LANE_LABEL[lane.lane]}
            </span>
            <div className='relative h-5 min-w-0 flex-1 overflow-hidden bg-muted/40'>
              {lane.lane === "production" &&
                data.releases.map((r) => {
                  const left = at(r.counted_from);
                  const right = at(r.ended_at ?? until);
                  const tone = ADMIN_TONE[verdictTone(r.verdict.kind)];
                  return (
                    <span
                      key={r.index}
                      className={cn(
                        "absolute inset-y-0 border-background border-l",
                        r.unpublished ? "bg-transparent" : tone.bg
                      )}
                      style={{ left: `${left}%`, width: `${Math.max(right - left, 0.5)}%` }}
                      data-testid={`admin-app-releases-segment-${r.index}`}
                    />
                  );
                })}
              {lane.events.map((e) => (
                <LaneMark key={e.id} event={e} left={at(e.at)} />
              ))}
            </div>
          </div>
        ))}
      {hidden && (
        <p
          className='text-muted-foreground text-xs'
          data-testid='admin-app-releases-nonprod-hidden'
        >
          Staging and sandboxes are not shown: you can't open this app's non-production
          environments.
        </p>
      )}
      <div className='flex gap-2 text-[10px] text-muted-foreground'>
        <span className='w-18 shrink-0' />
        <span className='flex min-w-0 flex-1 justify-between'>
          <span>{new Date(since).toLocaleDateString()}</span>
          <span>now</span>
        </span>
      </div>
    </div>
  );
};

/** One pointer move: a dot for a move to a build, a ring for one whose build was
 *  pruned, a bar for an unpublish. */
const LaneMark = ({ event, left }: { event: ReleaseLaneEvent; left: number }) => {
  const shape =
    event.action === "unpublish"
      ? "h-4 w-0.5 rounded-none bg-foreground/70"
      : event.pruned
        ? "size-2 rounded-full ring-1 ring-foreground/60"
        : "size-2 rounded-full bg-foreground/80";
  const build = event.build ? releaseLabel(event.build.build_id) : event.pruned ? "pruned" : "—";
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span
          className={cn("absolute top-1/2 -translate-x-1/2 -translate-y-1/2", shape)}
          style={{ left: `${left}%` }}
          data-testid={`admin-app-releases-event-${event.id}`}
        />
      </TooltipTrigger>
      <TooltipContent className={`${DESIGN_REFERENCE} text-xs`}>
        {event.environment} · {event.action} · {build} · {relativeTime(event.at)}
        {event.actor ? ` · ${event.actor}` : ""}
        {event.agent_built ? " · agent-built" : ""}
      </TooltipContent>
    </Tooltip>
  );
};
