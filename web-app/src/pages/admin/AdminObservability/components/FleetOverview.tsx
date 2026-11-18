import { Coins } from "lucide-react";
import { useMemo, useState } from "react";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import { AdminInfo } from "@/pages/admin/components/AdminInfo";
import type { Fleet } from "@/services/api/adminObservability";
import { AdminEmptyState } from "../../components/AdminEmptyState";
import { ADMIN_TONE } from "../../components/adminTone";
import {
  type FleetSummary,
  isCostJump,
  matchesQuery,
  priorNote,
  summarizeFleet
} from "../fleetSummary";
import { money, pct, priorAmount } from "../format";
import { RELIABILITY_DANGER_AT, reliabilityTone } from "../reliabilityTone";
import { CompositionKey } from "./CompositionBar";
import { CostUsageMap } from "./CostUsageMap";
import { IdleList, RankedList } from "./RankedList";

/** Rows the list shows before it folds the rest. */
export const LIST_TOP = 11;

/**
 * The fleet, ranked by cost to serve: one sentence that states the total, a
 * map of cost against work, and the ranked list beside it.
 *
 * Built for ~100 orgs. The map holds every active org at any count; the list
 * shows the top of the ranking and folds the long tail into one line; idle
 * orgs are counted, not drawn — the question this page answers first is who
 * costs most, and forty rows of $0 would bury it.
 */
export const FleetOverview = ({ fleet, query }: { fleet: Fleet; query: string }) => {
  const summary = useMemo(() => summarizeFleet(fleet), [fleet]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [expanded, setExpanded] = useState(false);
  const [showIdle, setShowIdle] = useState(false);

  // Carry the window: a 7- or 90-day list must not land on the 30-day console
  // and refetch everything for the default.
  const hrefFor = (orgId: string) =>
    `${ROUTES.ADMIN.OBSERVABILITY_ORG(orgId)}?days=${fleet.window_days}`;

  const searching = query.trim() !== "";
  const matched = summary.ranked.filter((o) => matchesQuery(o.row, query));
  const idleMatched = summary.idle.filter((r) => matchesQuery(r, query));
  const listed = searching || expanded ? matched : matched.slice(0, LIST_TOP);
  const folded = matched.slice(LIST_TOP);
  const foldedTotal = folded.reduce((s, o) => s + o.row.cost_to_serve_usd, 0);
  const scaleMax = summary.ranked[0]?.row.cost_to_serve_usd ?? 0;
  const unpriced = fleet.rows.reduce((s, r) => s + r.unpriced_runs, 0);

  return (
    <div className='space-y-3' data-testid='admin-observability-fleet'>
      <FleetSentence summary={summary} days={fleet.window_days} />

      {summary.ranked.length === 0 ? (
        <AdminEmptyState
          icon={Coins}
          title='Nothing cost anything in this window.'
          description={`${summary.idle.length} organization${summary.idle.length === 1 ? " is" : "s are"} idle: no LLM spend, no runs and no app traffic.`}
          data-testid='admin-observability-fleet-empty'
        />
      ) : (
        <div className='grid items-start gap-4 lg:grid-cols-[minmax(0,1.6fr)_minmax(22rem,1fr)]'>
          <section className='rounded-lg border border-border/60 bg-card'>
            <header className='flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1 border-border/60 border-b px-4 py-2.5'>
              <h3 className='flex items-center gap-1.5 font-semibold text-sm'>
                Cost against work
                <AdminInfo>
                  Both axes are log scale. Diagonals are cost per run; higher and further left costs
                  more for what it does.
                </AdminInfo>
              </h3>
            </header>
            <div className='px-2 pt-3 pb-1'>
              <CostUsageMap
                ranked={summary.ranked}
                activeId={activeId}
                onActive={setActiveId}
                matches={(o) => matchesQuery(o.row, query)}
                hrefFor={hrefFor}
              />
            </div>
            <div className='flex flex-wrap items-center justify-between gap-x-4 gap-y-1 border-border/60 border-t px-4 py-2'>
              <CompositionKey computePriced={fleet.compute_priced} />
              <StatusKey summary={summary} />
            </div>
          </section>

          <section className='rounded-lg border border-border/60 bg-card'>
            <header className='flex items-baseline justify-between gap-4 border-border/60 border-b px-4 py-2.5'>
              <h3 className='font-semibold text-sm'>Highest cost to serve</h3>
              <span className='text-muted-foreground text-xs tabular-nums'>
                of {money(summary.total)}
              </span>
            </header>
            {listed.length === 0 ? (
              <p
                className='px-4 py-6 text-center text-muted-foreground text-xs'
                data-testid='admin-observability-ranked-none'
              >
                No active organization matches &ldquo;{query.trim()}&rdquo;.
              </p>
            ) : (
              <RankedList
                rows={listed}
                scaleMax={scaleMax}
                activeId={activeId}
                onActive={setActiveId}
                hrefFor={hrefFor}
              />
            )}
            {!searching && folded.length > 0 ? (
              <Fold
                testId='admin-observability-ranked-fold'
                text={
                  expanded
                    ? `Showing all ${matched.length} active organizations.`
                    : `${folded.length} more active, ${money(foldedTotal)} between them.`
                }
                action={expanded ? `Top ${LIST_TOP}` : `Show all ${matched.length}`}
                onClick={() => setExpanded((v) => !v)}
              />
            ) : null}
            {idleMatched.length > 0 ? (
              <>
                <Fold
                  testId='admin-observability-idle-fold'
                  text={`${idleMatched.length} idle: no spend, runs or app traffic.`}
                  action={showIdle || searching ? null : "Show"}
                  onClick={() => setShowIdle((v) => !v)}
                />
                {showIdle || searching ? <IdleList rows={idleMatched} hrefFor={hrefFor} /> : null}
              </>
            ) : null}
          </section>
        </div>
      )}

      <FleetNotes fleet={fleet} unpriced={unpriced} />
    </div>
  );
};

/**
 * The page's first answer, in one sentence: the total, which way it moved,
 * and how concentrated it is. Concentration is the fact a ranked list implies
 * but never says — "three orgs carry half" is the margin conversation.
 */
const FleetSentence = ({ summary, days }: { summary: FleetSummary; days: number }) => {
  const { ranked, idle, total, versus, toHalf, toEighty } = summary;
  const n = ranked.length;
  return (
    <p
      className='max-w-4xl text-muted-foreground text-xs'
      data-testid='admin-observability-sentence'
    >
      <Em>{money(total)}</Em> to serve{" "}
      <Em>
        {n.toLocaleString()} active org{n === 1 ? "" : "s"}
      </Em>{" "}
      over the last {days} days
      {versus == null ? (
        ". "
      ) : versus.kind === "new" ? (
        <>
          , against {priorAmount(versus.prior)} the {days} days before.{" "}
        </>
      ) : versus.kind === "from" ? (
        <>
          , <Em>up from {money(versus.prior)}</Em> the {days} days before.{" "}
        </>
      ) : (
        <>
          ,{" "}
          <Em>
            {versus.change >= 0 ? "up" : "down"} {Math.abs(Math.round(versus.change * 100))}%
          </Em>{" "}
          on the {days} days before.{" "}
        </>
      )}
      {n >= 3 && toHalf != null && toEighty != null ? (
        <>
          <Em>
            {toHalf} org{toHalf === 1 ? "" : "s"}
          </Em>{" "}
          carr{toHalf === 1 ? "ies" : "y"} half of it; <Em>{toEighty}</Em> carr
          {toEighty === 1 ? "ies" : "y"} 80%.{" "}
        </>
      ) : null}
      {idle.length > 0 ? (
        <>
          {idle.length} idle org{idle.length === 1 ? " is" : "s are"} set aside.
        </>
      ) : null}
    </p>
  );
};

/**
 * What the map's two marks of colour mean — named once, and only when the
 * fleet has an org that wears one, since colour here always means a problem.
 */
const StatusKey = ({ summary }: { summary: FleetSummary }) => {
  const jumps = summary.ranked.filter(isCostJump).length;
  const failing = summary.ranked.filter(
    (o) => reliabilityTone(o.row.failure_rate) === "danger"
  ).length;
  if (jumps === 0 && failing === 0) return null;
  return (
    <ul
      className='flex flex-wrap items-center gap-x-3 gap-y-1 text-muted-foreground text-xs'
      data-testid='admin-observability-status-key'
    >
      {jumps > 0 ? (
        <li className='flex items-center gap-1.5'>
          <span
            aria-hidden
            className={cn("inline-block size-2 rounded-full", ADMIN_TONE.warn.dot)}
          />
          Cost more than doubled ({jumps})
        </li>
      ) : null}
      {failing > 0 ? (
        <li className='flex items-center gap-1.5'>
          <span
            aria-hidden
            className='inline-block size-2.5 rounded-full border border-destructive'
          />
          App errors at {pct(RELIABILITY_DANGER_AT)} or more ({failing})
        </li>
      ) : null}
    </ul>
  );
};

const Em = ({ children }: { children: React.ReactNode }) => (
  <span className='font-semibold text-foreground tabular-nums'>{children}</span>
);

const Fold = ({
  text,
  action,
  onClick,
  testId
}: {
  text: string;
  action: string | null;
  onClick: () => void;
  testId: string;
}) => (
  <div
    className='flex items-center justify-between gap-3 border-border/60 border-t px-4 py-1.5 text-muted-foreground text-xs'
    data-testid={testId}
  >
    <span>{text}</span>
    {action ? (
      // A text action, not a `Button`: the shared button sets its own type
      // size, and a fold's control should read at the fold's size.
      <button
        type='button'
        onClick={onClick}
        className='rounded-sm px-1 font-medium text-foreground underline-offset-2 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50'
      >
        {action}
      </button>
    ) : null}
  </div>
);

/**
 * What the totals do not include, said once under the page rather than as a
 * dash in every row.
 */
const FleetNotes = ({ fleet, unpriced }: { fleet: Fleet; unpriced: number }) => {
  const notes: { key: string; text: string; warn?: boolean }[] = [];
  if (!fleet.storage_read) {
    notes.push({
      key: "storage",
      warn: true,
      text: "Couldn’t read storage for this window, so every total leaves storage out — not because nothing was stored."
    });
  }
  if (!fleet.apps_measured) {
    notes.push({
      key: "apps",
      text: "Custom-app traffic is not measured on this deployment, so totals carry no egress or compute."
    });
  }
  if (!fleet.prior_read) {
    notes.push({ key: "prior", text: priorNote(fleet) });
  }
  if (unpriced > 0) {
    notes.push({
      key: "unpriced",
      text: `${unpriced.toLocaleString()} run${unpriced === 1 ? "" : "s"} used a model with no known price — counted as runs, not in dollars.`
    });
  }
  if (notes.length === 0) return null;
  return (
    <ul className='space-y-0.5 text-xs' data-testid='admin-observability-fleet-notes'>
      {notes.map((n) => (
        <li
          key={n.key}
          data-testid={`admin-observability-note-${n.key}`}
          className={cn(n.warn ? ADMIN_TONE.warn.text : "text-muted-foreground")}
        >
          {n.text}
        </li>
      ))}
    </ul>
  );
};
