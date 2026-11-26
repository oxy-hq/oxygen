import { useMemo } from "react";
import { cn } from "@/libs/shadcn/utils";
import type { Fleet } from "@/services/api/adminObservability";
import { ADMIN_TONE } from "../../components/adminTone";
import { isCostJump, summarizeFleet } from "../fleetSummary";
import { money, pct, priorAmount, signedPct, versusPrior } from "../format";
import { COST_PARTS, CompositionBar } from "./CompositionBar";

/**
 * Where this org stands: its cost to serve, which way it moved, and its place
 * in the fleet — the console's answer to the question the fleet list opened
 * with, before the panels below say why.
 *
 * Read from the fleet response rather than a second per-org endpoint, so the
 * number here is the number on the list the operator just clicked from.
 */
export const OrgStanding = ({ orgId, fleet }: { orgId: string; fleet: Fleet }) => {
  const summary = useMemo(() => summarizeFleet(fleet), [fleet]);
  const ranked = summary.ranked.find((o) => o.row.org_id === orgId);
  const row = ranked?.row ?? fleet.rows.find((r) => r.org_id === orgId);
  if (!row) return null;
  const days = fleet.window_days;
  // Off the row, not `ranked`: an idle org is not ranked but still has a prior.
  const versus = versusPrior(row.cost_to_serve_usd, row.prior_cost_to_serve_usd);
  const tone = ranked && isCostJump(ranked) ? ADMIN_TONE.warn.text : "text-foreground";

  return (
    <section
      className='space-y-3 border border-border/60 bg-card p-4'
      data-testid='admin-observability-standing'
    >
      <p className='text-muted-foreground text-xs'>
        <Em>{money(row.cost_to_serve_usd)}</Em> to serve over the last {days} days
        {versus == null ? (
          ". "
        ) : versus.kind === "new" ? (
          <>
            , against {priorAmount(versus.prior)} the {days} days before.{" "}
          </>
        ) : versus.kind === "from" ? (
          <>
            ,{" "}
            <span className={cn("font-semibold tabular-nums", tone)}>
              up from {money(versus.prior)}
            </span>{" "}
            the {days} days before.{" "}
          </>
        ) : (
          <>
            ,{" "}
            <span className={cn("font-semibold tabular-nums", tone)}>
              {signedPct(versus.change)}
            </span>{" "}
            on the {days} days before ({money(row.prior_cost_to_serve_usd)}).{" "}
          </>
        )}
        {ranked ? (
          <>
            <Em>
              #{ranked.rank} of {summary.ranked.length}
            </Em>{" "}
            active orgs, <Em>{pct(ranked.share)}</Em> of the fleet&rsquo;s cost to serve.
          </>
        ) : (
          "Idle in this window: no spend, runs or app traffic."
        )}
      </p>
      {row.cost_to_serve_usd > 0 ? (
        <CompositionBar parts={row} scaleMax={row.cost_to_serve_usd} className='h-3' />
      ) : null}
      <dl
        className='flex flex-wrap gap-x-6 gap-y-1 text-xs'
        data-testid='admin-observability-standing-parts'
      >
        {COST_PARTS.map((p) => {
          const v = p.value(row);
          const missing =
            p.key === "compute"
              ? "unpriced"
              : p.key === "storage"
                ? fleet.storage_read
                  ? "no readings"
                  : "unreadable"
                : "not measured";
          return (
            <div key={p.key} className='flex items-center gap-1.5'>
              <span aria-hidden className={cn("inline-block h-2 w-2.5 rounded-xs", p.fill)} />
              <dt className='text-muted-foreground'>{p.label}</dt>
              <dd className='font-medium tabular-nums'>
                {v == null ? (
                  <span className='font-normal text-muted-foreground'>{missing}</span>
                ) : (
                  money(v)
                )}
              </dd>
            </div>
          );
        })}
      </dl>
    </section>
  );
};

const Em = ({ children }: { children: React.ReactNode }) => (
  <span className='font-semibold text-foreground tabular-nums'>{children}</span>
);
