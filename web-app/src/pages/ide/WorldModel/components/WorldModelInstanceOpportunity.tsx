import { useMemo, useState } from "react";
import { useMetricTree } from "@/hooks/api/useMetricTree";
import type { OpportunityInstance } from "@/types/metricTree";
import type { WmSelection, WorldModel } from "@/types/worldModel";
import { declaringView } from "./measureTarget";
import {
  DEFAULT_PRESET_DAYS,
  WorldModelOpportunitiesSection
} from "./WorldModelOpportunitiesSection";
import { WorldModelPeerCohortSection } from "./WorldModelPeerCohortSection";

interface WorldModelInstanceOpportunityProps {
  measureName: string;
  /** True when the measure is promoted to this grain rather than declared on it. */
  induced: boolean;
  /** Source view of an induced measure — where it is actually declared. */
  promotedFrom?: string;
  /** `view` of the host entity, i.e. where a non-induced measure is declared. */
  entityView: string | undefined;
  /** `additive` | `non_additive` | `passthrough`. */
  additivity: string;
  /** The instance in focus — the scan is scoped to it. */
  instance: OpportunityInstance;
  /** The graph, for resolving a segment header back to a selectable node. */
  model: WorldModel;
  onSelect: (s: WmSelection) => void;
  /** The measure's `default_cohort:` from the world-model graph. Absent means
   *  no peer comparison is declared, and the peer section is not offered. */
  defaultCohort?: string | null;
}

/**
 * Opportunity sizing for one computed measure listed under a selected instance.
 *
 * Scoped to the instance in focus, so it answers "within this store, where is
 * the upside?" — the question the surrounding panel is already asking, and the
 * one that matches the instance-scoped measure value shown directly above it.
 * It resolves the world-model `(entity, measure)` address to a metric-tree node
 * the same way `WorldModelMeasureAnalysis` does.
 *
 * The scope reshapes the answer, by design: dimensions the instance pins to a
 * single value (this store's own name, city, region) have no peers left to
 * benchmark against and drop out, leaving the cuts that still vary inside the
 * instance — order status, customer and shipping attributes. The population-wide
 * "which store lags?" question still has a home: it is what the measure panel
 * (`WorldModelMeasureAnalysis`) asks, unscoped.
 *
 * Unlike the measure panel the OPPORTUNITIES half renders **nothing at all** —
 * no spinner, no "no node" copy — when the measure has no metric-tree node, so
 * an instance's measure list is not littered with empty toggles. It also passes
 * `hideWhenEmpty` so the section renders nothing when the sizing query comes
 * back with no data: an instance's measure list should never show an
 * "Opportunities" toggle that only opens onto a "no data" line. That resolution
 * is eager and costs one warehouse sizing scan per mounted measure (5-min
 * cached). Being scoped, that cache is per instance rather than shared across
 * them, so the cost is paid per instance visited.
 *
 * ## The second section
 *
 * [`WorldModelPeerCohortSection`] sits below it and answers the OTHER question
 * — this instance against **other instances of the same entity**. The two are
 * kept apart deliberately: the scoping described above is exactly what a domain
 * expert misread, taking the opportunities list for a cross-restaurant
 * comparison. Merging peer rows into it would have made that reading permanent.
 *
 * The peer section is gated differently, and that is the point rather than an
 * oversight. `sizable` and `drillable` answer "can the ENGINE decompose this
 * measure?"; whether a measure has a peer cohort is a fourth, unrelated
 * predicate — a `cohorts:` block on the entity plus `default_cohort:` on the
 * measure. Reusing the sizing gate would hide peer comparison for measures that
 * declare one perfectly well (a plain `count`, a composite the engine refuses),
 * so the peer section renders wherever the measure declares a `default_cohort:`
 * (carried on the world-model graph) on THIS instance's entity — whether or not
 * a metric-tree node exists. It costs nothing until expanded — see its own docs
 * for why it is lazy where this one is eager.
 */
export function WorldModelInstanceOpportunity({
  measureName,
  induced,
  promotedFrom,
  entityView,
  additivity,
  instance,
  model,
  onSelect,
  defaultCohort
}: WorldModelInstanceOpportunityProps) {
  const view = declaringView(induced, promotedFrom, entityView);
  const { data: tree } = useMetricTree();
  // Period preference is owned here so it survives the section resetting its
  // transient result state; the segment section requires it (an undefined
  // period would build an invalid date range and throw "Invalid time value").
  const [periodDays, setPeriodDays] = useState(DEFAULT_PRESET_DAYS);

  const node = useMemo(() => {
    if (!tree || !view) return undefined;
    return tree.nodes.find((n) => n.view === view && n.measure === measureName);
  }, [tree, view, measureName]);

  // Without a declaring view the measure has no `view.measure` address, so
  // neither section can form a request.
  if (!view) return null;

  // Only additive sums and non-additive rates are sizable; passthrough is not.
  // But `sizable` alone is NOT the surviving gate after the two sections merged
  // into one. Every engine-accepted composite (e.g. `checks.net_revenue`,
  // `checks.gross_profit`) classifies as `passthrough` here while the node's own
  // `drillable` is true — the drill section used to reach those measures because
  // it mounted unconditionally and self-gated. With one section left, gating on
  // `sizable` alone would silently drop the affordance for exactly the composites
  // the decomposition exists to expose, so `drillable` is admitted alongside it.
  const sizable = additivity === "additive" || additivity === "non_additive";
  const drillEnabled = (node?.drillable ?? false) || additivity === "additive";
  // Sizing needs a metric-tree node; peer comparison does not — see the note on
  // the fourth predicate above.
  const showSizing = !!node && (sizable || drillEnabled);
  // The peer section's own predicate: does the MODEL declare a comparison for
  // this measure? A toggle that opens onto "no peer cohort is declared" spends
  // a request to say nothing, and `hideWhenEmpty` above exists precisely so an
  // instance's measure list is not littered with empty toggles. Gating on the
  // declaration rather than on the resolution keeps the explanatory copy for
  // the case that is a real authoring error — a `default_cohort:` pointing at a
  // `cohorts:` block that is missing, or not in `entity.cohort` form, still
  // resolves to `cohort_not_resolved`, and the reader needs to be told.
  //
  // A declaration naming ANOTHER entity is not that error: the measure is
  // listed on every entity it is promoted to, all carrying the same
  // `default_cohort`, and only the named entity can resolve it. Elsewhere the
  // toggle would always fail and its copy would tell a correct model to
  // re-point its cohort — so it is hidden, mirroring `resolve_cohort_name`.
  const showPeers = !!defaultCohort && cohortBelongsTo(defaultCohort, instance.entity);

  return (
    <>
      {showSizing && node && (
        <WorldModelOpportunitiesSection
          nodeId={node.id}
          view={node.view}
          additivity={additivity}
          periodDays={periodDays}
          onPeriodDaysChange={setPeriodDays}
          instance={instance}
          model={model}
          onSelect={onSelect}
          hideWhenEmpty
        />
      )}
      {showPeers && (
        <WorldModelPeerCohortSection
          measure={`${view}.${measureName}`}
          view={view}
          instance={instance}
          periodDays={periodDays}
        />
      )}
    </>
  );
}

/** False only when `default_cohort` names an entity other than `entity`. A bare
 *  (malformed) name names none, so it is kept for the server to refuse. */
function cohortBelongsTo(defaultCohort: string, entity: string): boolean {
  const dot = defaultCohort.indexOf(".");
  return dot < 0 || defaultCohort.slice(0, dot) === entity;
}
