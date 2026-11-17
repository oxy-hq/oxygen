import { ArrowDown, ArrowUp, Minus } from "lucide-react";
import { Link } from "react-router-dom";
import { cn } from "@/libs/shadcn/utils";
import type { FleetRow } from "@/services/api/adminObservability";
import { ADMIN_TONE } from "../../components/adminTone";
import { isCostJump, type RankedOrg } from "../fleetSummary";
import { money, pct, signedPct } from "../format";
import { reliabilityTone } from "../reliabilityTone";
import { CompositionBar } from "./CompositionBar";

/**
 * The fleet in rank order, beside the map.
 *
 * Rows are `<Link>`s (cmd-click, copy link address), so this is a grid and
 * not a `<table>` — and per `pages/admin/CLAUDE.md`, one grid container with
 * header and rows as `grid-cols-subgrid` children, never a template string
 * repeated per row, which resolves a different layout for every row.
 *
 * Every bar shares one scale, the costliest org's total, so the column of
 * bars is itself a bar chart of the fleet.
 */
export const RankedList = ({
  rows,
  scaleMax,
  activeId,
  onActive,
  hrefFor
}: {
  rows: RankedOrg[];
  scaleMax: number;
  activeId: string | null;
  onActive: (id: string | null) => void;
  hrefFor: (orgId: string) => string;
}) => (
  <div
    className='grid grid-cols-[2rem_2.25rem_minmax(0,1fr)_max-content_max-content]'
    data-testid='admin-observability-ranked'
  >
    <div className='col-span-full grid grid-cols-subgrid border-border/60 border-b px-3 py-1.5 text-[11px] text-muted-foreground'>
      <span className='text-right'>#</span>
      <span className='sr-only'>Movement</span>
      <span className='pl-2'>Organization</span>
      <span className='pl-3 text-right'>Cost</span>
      <span className='pl-3 text-right'>Share</span>
    </div>
    {rows.map((o) => {
      const id = o.row.org_id;
      const tone = reliabilityTone(o.row.failure_rate);
      return (
        <Link
          key={id}
          to={hrefFor(id)}
          onMouseEnter={() => onActive(id)}
          onMouseLeave={() => onActive(null)}
          onFocus={() => onActive(id)}
          onBlur={() => onActive(null)}
          data-testid='admin-observability-ranked-row'
          className={cn(
            "col-span-full grid grid-cols-subgrid items-center border-border/40 border-b px-3 py-1.5 text-xs transition-colors last:border-b-0",
            id === activeId ? "bg-muted/60" : "hover:bg-muted/40",
            isCostJump(o) && ADMIN_TONE.warn.bg
          )}
        >
          <span className='text-right text-muted-foreground tabular-nums'>{o.rank}</span>
          <Movement movement={o.movement} />
          <div className='min-w-0 space-y-1 pl-2'>
            <div className='flex min-w-0 items-baseline gap-1.5'>
              <span className='truncate font-medium'>{o.row.org_name}</span>
              {tone !== "muted" ? (
                <span className={cn("shrink-0 text-[10px]", ADMIN_TONE[tone].text)}>
                  {pct(o.row.failure_rate, 1)} app errors
                </span>
              ) : null}
            </div>
            <CompositionBar parts={o.row} scaleMax={scaleMax} />
          </div>
          <div className='pl-3 text-right tabular-nums'>
            <div className='font-medium'>{money(o.row.cost_to_serve_usd)}</div>
            {o.change != null ? (
              <div
                className={cn(
                  "text-[10px]",
                  isCostJump(o) ? ADMIN_TONE.warn.text : "text-muted-foreground"
                )}
              >
                {signedPct(o.change)}
              </div>
            ) : null}
          </div>
          <span className='pl-3 text-right text-muted-foreground tabular-nums'>{pct(o.share)}</span>
        </Link>
      );
    })}
  </div>
);

/**
 * Places gained or lost since the prior window. A climb is drawn in the
 * foreground and a fall muted: the list is about who costs most, so moving up
 * it is the news. Neither is a colour — moving is not wrong.
 */
const Movement = ({ movement }: { movement: RankedOrg["movement"] }) => {
  if (movement == null) return <span aria-hidden />;
  if (movement === "new") {
    return (
      <span
        className='text-center text-[10px] text-muted-foreground'
        title='Cost nothing in the prior window'
      >
        new
      </span>
    );
  }
  if (movement === 0) {
    return (
      <span className='text-center text-muted-foreground' title='No change in rank'>
        <Minus aria-hidden className='mx-auto size-3 opacity-50' />
        <span className='sr-only'>No change in rank</span>
      </span>
    );
  }
  const up = movement > 0;
  const Icon = up ? ArrowUp : ArrowDown;
  return (
    <span
      className={cn(
        "flex items-center justify-center gap-0.5 text-[10px] tabular-nums",
        up ? "text-foreground" : "text-muted-foreground"
      )}
      title={`${up ? "Up" : "Down"} ${Math.abs(movement)} since the prior window`}
    >
      <Icon className='size-3' aria-hidden />
      <span className='sr-only'>{up ? "Up" : "Down"}</span>
      {Math.abs(movement)}
    </span>
  );
};

/** Idle orgs, named — a quiet tenant is a finding, so it is listed, not dropped. */
export const IdleList = ({
  rows,
  hrefFor
}: {
  rows: FleetRow[];
  hrefFor: (orgId: string) => string;
}) => (
  <ul
    className='flex flex-wrap gap-x-3 gap-y-1 px-3 py-2 text-xs'
    data-testid='admin-observability-idle'
  >
    {rows.map((r) => (
      <li key={r.org_id}>
        <Link
          to={hrefFor(r.org_id)}
          className='text-muted-foreground underline-offset-2 hover:text-foreground hover:underline'
        >
          {r.org_name}
        </Link>
      </li>
    ))}
  </ul>
);
