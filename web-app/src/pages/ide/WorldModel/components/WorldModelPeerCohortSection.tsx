import { ChevronDown, ChevronRight } from "lucide-react";
import { useMemo, useState } from "react";
import { useCohortQuery, useTimeDimensions } from "@/hooks/api/useMetricTree";
import type { CohortRequest, CohortSubject, OpportunityInstance } from "@/types/metricTree";
import { SectionHeader, SectionSpinner } from "../../components/semanticGraph";
import { formatCompact, formatDelta } from "./measureTarget";
import { presetPeriod } from "./WorldModelOpportunitiesSection";

/** Server code for "no cohort named and the measure declares no
 *  `default_cohort:`". An authoring gap, not a failure — see
 *  `crates/app/src/server/api/cohort/error.rs`. */
const NOT_DECLARED = "cohort_not_resolved";

interface WorldModelPeerCohortSectionProps {
  /** Fully-qualified `view.measure` to compare across instances. */
  measure: string;
  /** View the measure is declared on — where its time dimensions live. */
  view: string;
  /** The instance in focus: which entity, and which key to pick out of the
   *  population-wide response. */
  instance: OpportunityInstance;
  /** Trailing window, in days, shared with the Opportunities section above so
   *  the two sections describe the same period. */
  periodDays: number;
}

/**
 * "Compared to peers" — this instance benchmarked against **other instances of
 * the same entity**.
 *
 * Deliberately a section of its own, never merged into the Opportunities list
 * directly above it. The two answer different questions and the difference is
 * exactly what got misread in the field: `opportunity` with an `instance` scope
 * pins `restaurant_id = <this one>` onto every query, so its peer set is this
 * restaurant's own channels, dayparts and customer attributes — never other
 * restaurants. A domain expert read that list as "opportunities to increase
 * order count compared to other restaurants". It is not. Blending peer rows
 * under the same "addressable upside" heading would make that reading
 * unfalsifiable, so each section names the comparison it made.
 *
 * **Lazy, unlike the Opportunities section.** That one resolves eagerly to hide
 * itself when empty; a cohort resolution costs two entity-grain warehouse pulls
 * and an instance panel mounts one of these per computed measure, so the query
 * waits for a human to open the toggle.
 *
 * **Mounting is gated upstream**, in [`WorldModelInstanceOpportunity`], on the
 * measure's `default_cohort:` as carried by the world-model graph — schema-only,
 * so it costs nothing, and only where that declaration names this instance's
 * entity. So `cohort_not_resolved` here means a declaration `resolve_cohort_name`
 * refused — today, one not in `entity.cohort_name` form — and the copy below
 * shows the server's message rather than guessing at the fault. A declaration
 * naming a `cohorts:` block that does not exist gets past that check and fails
 * inside the resolution instead, as an ordinary error.
 *
 * **The time dimension is named** on the provenance line: this section filters
 * on the view's first one, while the Opportunities section lets the reader pick
 * another, so the two would otherwise look alike over different columns.
 *
 * Three response semantics this component exists to render, all easy to destroy
 * by tidying:
 *
 *  - **`excluded` is shown with its reason.** The predecessor implementation
 *    dropped an uncomparable store from the list with no screen saying why.
 *    If the instance in focus is itself excluded, the section says so — an
 *    empty section would be the same bug.
 *  - **`sufficient: false` is flagged, never filtered.** `min_peers` is a
 *    reporting predicate; a thin baseline is the reader's call to make.
 *  - **The cohort name, statistic and band window are echoed**, so what is on
 *    screen cannot drift from the query behind it.
 */
export function WorldModelPeerCohortSection({
  measure,
  view,
  instance,
  periodDays
}: WorldModelPeerCohortSectionProps) {
  const [open, setOpen] = useState(false);

  const {
    data: timeDims,
    isPending: timeDimsPending,
    error: timeDimsError
  } = useTimeDimensions({ enabled: open });
  const timeDim = timeDims?.by_view[view]?.[0] ?? "";

  const request = useMemo<CohortRequest | null>(() => {
    if (!open || !timeDim) return null;
    return {
      entity: instance.entity,
      measure,
      time_dimension: timeDim,
      period: presetPeriod(periodDays)
      // No `cohort`: omitting it resolves the measure's own `default_cohort:`,
      // which is the declaration this surface is meant to read.
    };
  }, [open, timeDim, instance.entity, measure, periodDays]);

  const cohort = useCohortQuery(request, open && !!request);

  const notDeclared = errorCode(cohort.error) === NOT_DECLARED;
  const data = cohort.data;
  const self = data ? findSubject(data.subjects, instance.key) : undefined;
  const selfExcluded = data
    ? data.excluded.find((e) => matchesInstanceKey(e.key, instance.key))
    : undefined;

  return (
    <section className='flex flex-col gap-1.5 border-border border-t pt-3'>
      <button
        type='button'
        onClick={() => setOpen((v) => !v)}
        className='flex w-full items-center gap-1 text-left'
        aria-expanded={open}
        data-testid={`wm-peers-toggle-${measure}`}
      >
        {open ? (
          <ChevronDown size={12} className='shrink-0 text-muted-foreground' />
        ) : (
          <ChevronRight size={12} className='shrink-0 text-muted-foreground' />
        )}
        <span className='min-w-0 flex-1'>
          <SectionHeader
            title='Compared to peers'
            // Names the comparison, in the entity's own words — the sentence
            // the Opportunities section above cannot truthfully say.
            subtitle={`this ${instance.entity} vs other ${instance.entity}s`}
            color='green'
          />
        </span>
      </button>

      {open && (
        <div className='flex flex-col gap-2 pt-1'>
          {timeDimsPending && <SectionSpinner />}

          {/* A failed lookup is shown as a failure: the cohort query waits on
              a time dimension, so its own error display never runs. */}
          {!!timeDimsError && (
            <p
              className='font-mono text-[10px] text-destructive leading-relaxed'
              data-testid='wm-peers-error'
            >
              {errorMessage(timeDimsError) ?? "Failed to load this view's time dimensions."}
            </p>
          )}

          {!timeDimsPending && !timeDimsError && !timeDim && (
            <p className='font-mono text-[10px] text-muted-foreground leading-relaxed'>
              No date or datetime dimension is declared on <code>{view}</code>, so there is no
              period to compare over.
            </p>
          )}

          {!!request && cohort.isPending && <SectionSpinner label='comparing against peers…' />}

          {notDeclared && (
            <p
              className='font-mono text-[10px] text-muted-foreground leading-relaxed'
              data-testid='wm-peers-undeclared'
            >
              The <code>default_cohort:</code> on <code>{measure}</code> could not be resolved
              {/* The server's message names the exact authoring fault; fixed
                  copy here would guess at it, and guessed wrong before. */}
              {errorMessage(cohort.error) ? <>: {errorMessage(cohort.error)}</> : "."}
            </p>
          )}

          {!!cohort.error && !notDeclared && (
            <p
              className='font-mono text-[10px] text-destructive leading-relaxed'
              data-testid='wm-peers-error'
            >
              {cohort.error instanceof Error
                ? cohort.error.message
                : "Failed to compare against peers."}
            </p>
          )}

          {!!data && (
            <>
              <p
                className='font-mono text-[9.5px] text-muted-foreground leading-relaxed'
                data-testid='wm-peers-provenance'
              >
                cohort <span className='text-foreground'>{data.cohort}</span> · {data.statistic} of
                peers · period {data.period[0]}…{data.period[1]} on{" "}
                <span className='text-foreground'>{timeDim}</span>
                {data.band_window != null && (
                  <>
                    {" "}
                    · band measured over {data.band_window[0]}…{data.band_window[1]}
                  </>
                )}
              </p>

              {self && <SelfRow subject={self} />}

              {!self && selfExcluded && (
                <p
                  className='font-mono text-[10px] text-muted-foreground leading-relaxed'
                  data-testid='wm-peers-self-excluded'
                >
                  This {instance.entity} was left out of the comparison:{" "}
                  <span className='text-foreground'>{selfExcluded.reason}</span>
                </p>
              )}

              {!self && !selfExcluded && (
                <p
                  className='font-mono text-[10px] text-muted-foreground leading-relaxed'
                  data-testid='wm-peers-self-missing'
                >
                  This {instance.entity} is in neither the compared nor the excluded set for this
                  period — the pull returned no row for key <code>{instance.key}</code>.
                </p>
              )}

              {data.excluded.length > 0 && (
                <div className='flex flex-col gap-0.5' data-testid='wm-peers-excluded'>
                  <span className='text-[9.5px] text-muted-foreground uppercase tracking-wider'>
                    Left out of the comparison
                  </span>
                  {data.excluded.map((e) => (
                    <p
                      key={e.key}
                      className='font-mono text-[10px] text-muted-foreground leading-relaxed'
                    >
                      <span className='text-foreground'>{e.key}</span> — {e.reason}
                    </p>
                  ))}
                </div>
              )}
            </>
          )}
        </div>
      )}
    </section>
  );
}

/** This instance's own comparison. `peer_count` sits beside the baseline
 *  rather than in a tooltip: a baseline drawn from two peers and one drawn
 *  from twenty are different claims, and the number is the only thing that
 *  says which. */
function SelfRow({ subject }: { subject: CohortSubject }) {
  return (
    <div className='flex flex-col gap-0.5' data-testid='wm-peers-self'>
      <div className='flex items-baseline justify-between gap-2 font-mono text-[10px]'>
        <span className='text-muted-foreground'>this instance</span>
        <span className='text-foreground'>{formatCompact(subject.value)}</span>
      </div>
      <div className='flex items-baseline justify-between gap-2 font-mono text-[10px]'>
        <span className='text-muted-foreground'>
          peer benchmark ({subject.peer_count} {subject.peer_count === 1 ? "peer" : "peers"})
        </span>
        <span className='text-foreground'>
          {/* `baseline` is 0 when there are no peers — the ABSENCE of a
              benchmark, not a benchmark of zero. Say so rather than printing a
              number that would read as a real comparison. */}
          {subject.peer_count === 0 ? "—" : formatCompact(subject.baseline)}
        </span>
      </div>
      {subject.peer_count > 0 && (
        <div className='flex items-baseline justify-between gap-2 font-mono text-[10px]'>
          <span className='text-muted-foreground'>gap (positive = upside)</span>
          <span className='text-foreground'>{formatDelta(subject.gap)}</span>
        </div>
      )}
      {!subject.sufficient && (
        <p
          className='font-mono text-[9.5px] text-warning leading-relaxed'
          data-testid='wm-peers-thin'
        >
          Thin cohort: {subject.peer_count} {subject.peer_count === 1 ? "peer" : "peers"}, below
          this cohort's <code>min_peers</code>. The comparison is shown as computed — judge it
          accordingly.
        </p>
      )}
    </div>
  );
}

/** The `code` a coded API error carries, if any.
 *
 *  Duck-typed rather than an `instanceof` check: the query client can hand back
 *  a rejected value of any shape, and the only thing that matters is whether a
 *  `code` came through. */
function errorCode(error: unknown): string | undefined {
  if (error && typeof error === "object" && "code" in error) {
    const code = (error as { code?: unknown }).code;
    return typeof code === "string" ? code : undefined;
  }
  return undefined;
}

/** The message a rejected query carries, if any — duck-typed like [`errorCode`]. */
function errorMessage(error: unknown): string | undefined {
  if (error && typeof error === "object" && "message" in error) {
    const message = (error as { message?: unknown }).message;
    return typeof message === "string" && message ? message : undefined;
  }
  return undefined;
}

function findSubject(subjects: CohortSubject[], instanceKey: string): CohortSubject | undefined {
  return subjects.find((s) => matchesInstanceKey(s.key, instanceKey));
}

/**
 * Whether a cohort subject key addresses the instance the panel is showing.
 *
 * The world-model API spells an instance key as a JSON array for a COMPOSITE
 * key and a bare scalar otherwise; airlayer keys a subject by the entity key's
 * value. So a one-element array is the same instance and must match. A genuinely
 * composite key matches nothing here, and the section says the instance is in
 * neither set — which is honest: a cohort over a composite-keyed entity is not
 * something this surface can address yet.
 */
function matchesInstanceKey(subjectKey: string, instanceKey: string): boolean {
  if (subjectKey === instanceKey) return true;
  try {
    const parsed: unknown = JSON.parse(instanceKey);
    if (Array.isArray(parsed) && parsed.length === 1) return String(parsed[0]) === subjectKey;
  } catch {
    // Not JSON — a bare scalar key, already compared above.
  }
  return false;
}
