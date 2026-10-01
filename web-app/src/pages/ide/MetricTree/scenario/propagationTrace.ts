// Turning `predict`'s `path` into the hop-by-hop account of how a number was
// reached. Pure — the panel renders what this returns and decides nothing about
// the model itself.

import type { DriverForm, FittedDriver, MetricEdge, MetricTree } from "@/types/metricTree";

/** Where a hop's coefficient came from, which is the difference between a
 *  number someone wrote down and a number measured off history.
 *
 *  - `declared` — the `.view.yml` states it. It is an assertion, not evidence.
 *  - `fitted` — the baseline regressed it over the window, so it carries `n`
 *    and can move when the window does.
 *  - `refused` — a fit was attempted and declined; `refusal` says why. The edge
 *    is a direction without a magnitude, and nothing is forecast across it.
 *  - `none` — a component edge, whose quantitative content is its `sign`, or a
 *    driver edge nothing tried to size.
 */
type CoefficientSource = "declared" | "fitted" | "refused" | "none";

/** One edge traversed on the way from a lever to an impacted measure. */
export interface TraceHop {
  from: string;
  to: string;
  /** `undefined` when the tree no longer contains this edge — a predict result
   *  outliving the tree it was computed against. The hop is still named; it
   *  just carries no metadata, which is honest about what is known. */
  edge?: MetricEdge;
  kind?: MetricEdge["kind"];
  /** Component edges only: `-1` means the child subtracts from its parent. */
  sign?: number;
  form?: DriverForm;
  /** False means the shape was inferred from history and can change with the
   *  window, so it is worth marking. */
  formDeclared?: boolean;
  lag?: number | null;
  coefficient?: number;
  coefficientSource: CoefficientSource;
  /** The fit behind a `fitted` or `refused` coefficient — carries `n`, the
   *  refusal text, and the rest of the evidence. */
  fit?: FittedDriver;
  description?: string | null;
}

function edgeKey(from: string, to: string): string {
  return `${from}\u0000${to}`;
}

/**
 * The chain of edges behind one propagated impact.
 *
 * `path` arrives from `predict` as `[lever, …, target]`, ids only. Everything
 * that makes a hop legible — its kind, form, coefficient, lag — lives on the
 * tree edge and on the baseline's fit, both of which the panel already holds,
 * so no request is needed to explain a number that has already been shown.
 */
export function buildTrace(
  path: string[] | undefined,
  tree: Pick<MetricTree, "edges"> | undefined,
  fitted: FittedDriver[] | undefined
): TraceHop[] {
  if (!path || path.length < 2) return [];

  const edges = new Map((tree?.edges ?? []).map((e) => [edgeKey(e.from, e.to), e]));
  const fits = new Map((fitted ?? []).map((f) => [edgeKey(f.from, f.to), f]));

  const hops: TraceHop[] = [];
  for (let i = 0; i < path.length - 1; i++) {
    const from = path[i];
    const to = path[i + 1];
    const edge = edges.get(edgeKey(from, to));
    const fit = fits.get(edgeKey(from, to));

    hops.push({
      from,
      to,
      edge,
      kind: edge?.kind,
      // Only meaningful on a component edge: a driver edge's sign lives in its
      // coefficient, and reporting a defaulted +1 there would invent a claim.
      sign: edge?.kind === "component" ? (edge.sign ?? 1) : undefined,
      form: edge?.kind === "driver" ? edge.form : undefined,
      formDeclared: edge?.kind === "driver" ? edge.form_declared : undefined,
      lag: edge?.lag,
      ...resolveCoefficient(edge, fit),
      description: edge?.description
    });
  }
  return hops;
}

/**
 * A hop's coefficient and where it came from.
 *
 * A declared coefficient wins over a fit: the baseline only fits edges that
 * declare none (see `BaselineResponse.fitted`), so if both are somehow present
 * the YAML is what propagation used.
 */
function resolveCoefficient(
  edge: MetricEdge | undefined,
  fit: FittedDriver | undefined
): { coefficient?: number; coefficientSource: CoefficientSource; fit?: FittedDriver } {
  if (edge?.kind === "component") return { coefficientSource: "none" };
  if (edge?.coefficient !== undefined && edge.coefficient !== null) {
    return { coefficient: edge.coefficient, coefficientSource: "declared" };
  }
  if (fit) {
    // `!= null`, per `FittedDriver.coefficient`'s contract: the field is an
    // `Option<f64>` on the wire, so an absent fit arrives as either encoding.
    return fit.coefficient != null
      ? { coefficient: fit.coefficient, coefficientSource: "fitted", fit }
      : { coefficientSource: "refused", fit };
  }
  return { coefficientSource: "none" };
}

/** The first hop whose edge carries no magnitude at all — the place an
 *  unquantifiable verdict was decided, when it was decided on this path.
 *
 *  `undefined` means the shown path is fully sized, which for an unquantifiable
 *  impact means the break is on a path `predict` did not return. That is worth
 *  saying rather than pointing at the wrong hop. */
export function unsizableHop(hops: TraceHop[]): TraceHop | undefined {
  return hops.find((h) => h.coefficientSource === "refused" || h.coefficientSource === "none");
}

/** Walk cap for `countPathsTo`. A metric tree is small, but it is a DAG, not a
 *  tree, and path counts multiply across diamonds — the panel only needs to
 *  know "one route or several", so counting stops as soon as that is settled
 *  plus a little headroom. Past the cap the count is a floor, not a total,
 *  which is why `capped` travels with it. */
const PATH_COUNT_CAP = 8;

/** The result of `countPathsTo`. `capped` means the walk stopped at
 *  `PATH_COUNT_CAP` with routes left unexplored, so `count` is a lower bound
 *  and must be rendered as "N+" rather than as an exact total. */
export interface PathCount {
  count: number;
  capped: boolean;
}

/** Whether a change can carry any magnitude across this edge.
 *
 *  A component edge always can — its content is arithmetic, badged as a sign.
 *  A driver edge can only when something sized it, declared or fitted; one that
 *  was refused or never sized contributes nothing to the summed figure, so
 *  counting it as a route the total sums would overstate what was added up. */
function carriesMagnitude(edge: MetricEdge, fit: FittedDriver | undefined): boolean {
  if (edge.kind === "component") return true;
  const { coefficientSource } = resolveCoefficient(edge, fit);
  return coefficientSource === "declared" || coefficientSource === "fitted";
}

/**
 * How many distinct routes carry a lever's change into `target`.
 *
 * Load-bearing, not trivia. `predict` sums the delta over every path into a
 * node but returns only the FIRST path's ids, so a trace presented as *the*
 * explanation of a two-route number is wrong by construction. Counting here is
 * what lets the panel say "this is one of two routes; the total sums both."
 *
 * Only routes that could contribute a magnitude are counted, because that is
 * the claim the panel makes about them — a route through an unsized driver
 * edge reaches the measure but adds nothing to the figure.
 *
 * Counts simple paths (no repeated node) from any lever, capped.
 */
export function countPathsTo(
  target: string,
  leverIds: string[],
  tree: Pick<MetricTree, "edges"> | undefined,
  fitted?: FittedDriver[]
): PathCount {
  if (!tree || leverIds.length === 0) return { count: 0, capped: false };

  const fits = new Map((fitted ?? []).map((f) => [edgeKey(f.from, f.to), f]));
  const fwd = new Map<string, string[]>();
  for (const edge of tree.edges) {
    if (!carriesMagnitude(edge, fits.get(edgeKey(edge.from, edge.to)))) continue;
    const list = fwd.get(edge.from);
    if (list) list.push(edge.to);
    else fwd.set(edge.from, [edge.to]);
  }

  // Walk to ONE past the cap, not to the cap. Stopping at the cap cannot tell
  // "exactly eight routes" from "eight and more to find" — the walk halts
  // either way — so `capped` would have to be set on the mere act of stopping,
  // reporting "8+" for a graph with exactly eight. Finding a ninth is what
  // proves there are more; the ninth is then dropped from the reported count,
  // which is why the cap is still the cap.
  let count = 0;
  const walk = (node: string, seen: Set<string>) => {
    if (count > PATH_COUNT_CAP) return;
    if (node === target) {
      count++;
      return;
    }
    for (const next of fwd.get(node) ?? []) {
      if (seen.has(next)) continue;
      seen.add(next);
      walk(next, seen);
      seen.delete(next);
    }
  };

  for (const lever of leverIds) {
    if (lever === target) continue;
    walk(lever, new Set([lever]));
  }
  return { count: Math.min(count, PATH_COUNT_CAP), capped: count > PATH_COUNT_CAP };
}
