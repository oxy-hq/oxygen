import { ChevronDown, ChevronRight } from "lucide-react";
import { useState } from "react";
import type { CandidateKind, DrillCandidate, DrillLevel, StopReason } from "@/types/metricTree";
import { InfoTip, MetaBadge } from "../../components/semanticGraph";
import { formatCompact, formatDelta, formatSignedPct, shortMeasureName } from "./measureTarget";
import { formatSegment } from "./worldModelNav";

/** The stop reason, in words, said where the chain ends. Every variant is
 *  spelled out: a bare `MaxDepth` reads as an error, and an unglossed
 *  `GateInconclusive` hides that the search chose to stop, not that it finished. */
const STOP_REASON_COPY: Record<StopReason, string> = {
  NoCandidates: "no further splits — the chain bottomed out",
  GateInconclusive: "stopped — the next split couldn't be proven",
  MaxDepth: "reached the depth limit",
  GateFailed: "the next split was within sampling noise"
};

const UNPROVEN_HELP =
  "This split's gap couldn't be told apart from sampling noise, so it's followed as the largest candidate but not proven. Confirm it against the data before acting on it.";

export const METHOD_HELP =
  "At each level the drill follows the SINGLE biggest cut of the gap — one dimension value or one component measure — and recurses into it. The other candidates are real cuts of the same gap, not additive alternatives; they are shown collapsed so 'follow the max' reads as the choice it is. The chain stops when no split can be proven above sampling noise, or at the depth limit.";

/** A candidate's human label: `dimension = value` for a segment split, the child
 *  measure's short name for a component split. */
function candidateLabel(kind: CandidateKind): string {
  if ("Dimension" in kind) {
    return `${kind.Dimension.dimension} = ${formatSegment(kind.Dimension.value)}`;
  }
  return shortMeasureName(kind.Component.measure);
}

/** Data-driven depth indent, like MagnitudeBar's width — kept off the fixed
 *  spacing scale (a class per possible depth is impossible) via an inline style. */
function indentStyle(depth: number): { marginLeft: string } {
  return { marginLeft: `${depth * 0.75}rem` };
}

/**
 * The headline plus the ordered chain.
 *
 * The engine's `levels[N]` describes the segment ENTERING level N (the result of
 * following level N-1's winner): `levels[N].gap`/`levels[N].root_share` are that
 * segment's magnitude and its cascaded share of the ROOT gap, so `levels[0]` is
 * the root itself (`root_share === 1`). The split CHOSEN at level N is
 * `levels[N].candidates[0]`, followed INTO level N+1 — so that split's own gap
 * and cascaded share are read from `levels[N+1]`, not `levels[N]`. Every level
 * but the last has `stop_reason === null` (it was followed); the last carries the
 * stop reason and its candidates were considered but NOT followed.
 */
export function DrillChain({
  levels,
  rootGap,
  rootUpside,
  idPrefix
}: {
  levels: DrillLevel[];
  rootGap: number;
  rootUpside: number;
  idPrefix: string;
}) {
  // Every level but the last is a followed split; the last is the terminal stop.
  const lastIndex = levels.length - 1;
  const followedCount = lastIndex;

  return (
    <div className='flex flex-col gap-2'>
      <p className='font-mono text-[10px] leading-relaxed'>
        <span className='text-muted-foreground'>Gap of </span>
        <span className='text-foreground'>{formatCompact(rootGap)}</span>
        <span className='text-info'> {formatDelta(rootUpside)} addressable</span>
        <span className='text-muted-foreground'>
          {followedCount > 0
            ? ` · followed down ${followedCount} split${followedCount === 1 ? "" : "s"}. `
            : " · couldn't be split further. "}
        </span>
        <InfoTip content={METHOD_HELP} />
      </p>
      <div className='flex flex-col gap-1.5'>
        {levels.map((level, n) => {
          if (n < lastIndex) {
            const next = levels[n + 1];
            return (
              <FollowedSplitRow
                // Ordered chain regenerated whole per query, never reordered — the
                // position IS the identity.
                // biome-ignore lint/suspicious/noArrayIndexKey: ordered chain, position is identity
                key={`followed-${n}`}
                // The split chosen AT level n; its own magnitude/share live in n+1.
                candidate={level.candidates[0]}
                siblings={level.candidates.slice(1)}
                rootShare={next.root_share}
                gap={next.gap}
                depth={n}
                idPrefix={idPrefix}
              />
            );
          }
          return (
            // biome-ignore lint/suspicious/noArrayIndexKey: ordered chain, position is identity
            <StopRow key={`stop-${n}`} level={level} depth={n} idPrefix={idPrefix} />
          );
        })}
      </div>
    </div>
  );
}

/**
 * One FOLLOWED split: the cut the drill recursed into, its cascaded share of the
 * root gap, the local share of its parent, the siblings it beat, and an unproven
 * caveat when the gate could not confirm it. Its magnitude (`gap`) and cascaded
 * `rootShare` are the NEXT level's — the segment this split produces.
 */
function FollowedSplitRow({
  candidate,
  siblings,
  rootShare,
  gap,
  depth,
  idPrefix
}: {
  candidate: DrillCandidate | undefined;
  siblings: DrillCandidate[];
  rootShare: number;
  gap: number;
  depth: number;
  idPrefix: string;
}) {
  // Defensive: a non-last level always has a followed candidate by the engine's
  // invariant, but never assume a split when the data lacks one.
  if (!candidate) return null;
  const rootPct = formatSignedPct(rootShare, 1);
  const localPct = formatSignedPct(candidate.concentration, 1);

  return (
    <div className='flex flex-col gap-0.5 border-info/30 border-l pl-2' style={indentStyle(depth)}>
      <div className='flex min-w-0 items-center justify-between gap-2 font-mono text-xs'>
        <span className='flex min-w-0 items-center gap-1.5'>
          <span className='truncate text-foreground'>{candidateLabel(candidate.kind)}</span>
          {!candidate.gated && <MetaBadge tooltip={UNPROVEN_HELP}>unproven</MetaBadge>}
        </span>
        <span className='flex shrink-0 items-baseline gap-1.5'>
          <span className='text-info'>{formatDelta(gap)}</span>
          {rootPct && (
            <span className='text-[9.5px] text-muted-foreground'>{rootPct} of root gap</span>
          )}
        </span>
      </div>
      {/* Below the root the local share (of the parent segment's gap) differs from
          the cascaded root share; show it so the reader sees the cascade multiply.
          At depth 0 the parent IS the root, so the two coincide — omit it. */}
      {depth > 0 && localPct && (
        <div className='font-mono text-[9.5px] text-muted-foreground'>
          {localPct} of the split above
        </div>
      )}
      {siblings.length > 0 && (
        <ConsideredSplits
          candidates={siblings}
          label={`${siblings.length} other split${siblings.length === 1 ? "" : "s"} considered`}
          testId={`wm-drill-siblings-${idPrefix}-${depth}`}
        />
      )}
    </div>
  );
}

/** The terminal level: why the chain stopped, plus the candidates that were
 *  considered there but NOT followed (for `GateInconclusive`, `candidates[0]` is
 *  the split the gate rejected — never rendered as an accepted step). */
function StopRow({
  level,
  depth,
  idPrefix
}: {
  level: DrillLevel;
  depth: number;
  idPrefix: string;
}) {
  const considered = level.candidates;

  return (
    <div className='flex flex-col gap-0.5 border-border border-l pl-2' style={indentStyle(depth)}>
      <div className='font-mono text-[9.5px] text-muted-foreground'>
        {level.stop_reason ? STOP_REASON_COPY[level.stop_reason] : "the chain ends here"}
      </div>
      {considered.length > 0 && (
        <ConsideredSplits
          candidates={considered}
          label={`${considered.length} split${considered.length === 1 ? "" : "s"} considered, not followed`}
          testId={`wm-drill-considered-${idPrefix}-${depth}`}
        />
      )}
    </div>
  );
}

/** A collapsed list of candidate cuts — the runner-up siblings under a followed
 *  split, or the considered-but-not-followed candidates at the stop. Each shows
 *  its gap and its share of THIS level's gap (`concentration`). */
function ConsideredSplits({
  candidates,
  label,
  testId
}: {
  candidates: DrillCandidate[];
  label: string;
  testId: string;
}) {
  const [open, setOpen] = useState(false);

  return (
    <div className='flex flex-col gap-0.5'>
      <button
        type='button'
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        data-testid={testId}
        className='flex items-center gap-1 text-left font-mono text-[9.5px] text-muted-foreground transition-colors hover:text-foreground'
      >
        {open ? <ChevronDown size={10} /> : <ChevronRight size={10} />}
        {label}
      </button>
      {open && (
        <div className='flex flex-col gap-0.5 pl-3'>
          {candidates.map((c, i) => {
            const pct = formatSignedPct(c.concentration, 1);
            return (
              <div
                // Fixed ranked list regenerated per query; label plus rank is stable.
                // biome-ignore lint/suspicious/noArrayIndexKey: fixed ranked list, not reordered
                key={`${candidateLabel(c.kind)}-${i}`}
                className='flex min-w-0 items-center justify-between gap-2 font-mono text-[9.5px]'
              >
                <span className='truncate text-muted-foreground'>{candidateLabel(c.kind)}</span>
                <span className='shrink-0 text-muted-foreground'>
                  {formatDelta(c.gap)}
                  {pct ? ` · ${pct} of level` : ""}
                </span>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
