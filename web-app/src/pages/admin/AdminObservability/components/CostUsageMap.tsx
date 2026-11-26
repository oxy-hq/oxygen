import { useLayoutEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { cn } from "@/libs/shadcn/utils";
import { isCostJump, type RankedOrg } from "../fleetSummary";
import { compact, money, versusShort } from "../format";
import {
  axisCount,
  axisUsd,
  decadeTicks,
  type LogDomain,
  logDomain,
  logPosition,
  perRunGuides,
  perRunLabel
} from "../mapScale";
import { reliabilityTone } from "../reliabilityTone";

/**
 * Every active org placed by what it costs to serve (up) against the agent
 * runs it did for that (across), both on log axes.
 *
 * The question the page leads with is "who costs most", and height answers it.
 * Width says whether that cost bought work: on log–log axes a constant cost
 * per run is a diagonal, so the guides read as "$0.50 a run" and an org above
 * the top one is expensive for what it does, whatever its size.
 *
 * Two gutters hold what a log axis cannot place, rather than dropping it: a
 * column at the left for orgs with cost but no runs (app traffic and storage
 * only), and a row at the bottom for orgs with runs but no priced cost.
 */
const M = { top: 14, right: 18, bottom: 30, left: 46 };
const GUTTER = 26;
/** The map is about as tall as the ranked list beside it, within reason. */
const heightFor = (width: number) => Math.min(560, Math.max(360, Math.round(width * 0.68)));
/** How many of the costliest orgs carry a name on the map. */
const LABELLED = 8;

export const CostUsageMap = ({
  ranked,
  activeId,
  onActive,
  matches,
  hrefFor
}: {
  ranked: RankedOrg[];
  activeId: string | null;
  onActive: (id: string | null) => void;
  /** Orgs the search keeps; the rest dim rather than vanish. */
  matches: (o: RankedOrg) => boolean;
  hrefFor: (orgId: string) => string;
}) => {
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    setWidth(el.getBoundingClientRect().width);
    const ro = new ResizeObserver(([entry]) => setWidth(entry.contentRect.width));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const xd: LogDomain = widen(logDomain(ranked.map((o) => o.row.run_count)) ?? [1, 10]);
  const yd: LogDomain = logDomain(ranked.map((o) => o.row.cost_to_serve_usd)) ?? [1, 10];

  const HEIGHT = heightFor(width);
  const plot = {
    x0: M.left + GUTTER,
    x1: Math.max(width - M.right, M.left + GUTTER + 1),
    y0: M.top,
    y1: HEIGHT - M.bottom - GUTTER
  };
  // The gutters' centres: where a zero sits on an axis that has no zero.
  const gutterX = M.left + (GUTTER - 4) / 2;
  const gutterY = plot.y1 + 4 + (GUTTER - 4) / 2;
  const x = (runs: number) =>
    runs > 0 ? plot.x0 + logPosition(runs, xd) * (plot.x1 - plot.x0) : gutterX;
  const y = (cost: number) =>
    cost > 0 ? plot.y1 - logPosition(cost, yd) * (plot.y1 - plot.y0) : gutterY;

  const guides = perRunGuides(
    ranked
      .filter((o) => o.row.run_count > 0 && o.row.cost_to_serve_usd > 0)
      .map((o) => o.row.cost_to_serve_usd / o.row.run_count)
  );

  const points = ranked.map((o) => ({ o, cx: x(o.row.run_count), cy: y(o.row.cost_to_serve_usd) }));
  // Guides are labelled first, each at the first spot along its line that
  // clears every dot; org names then avoid both the dots and those labels.
  const guideLabels = guides.flatMap((k) => {
    const seg = visibleSegment(k, xd, yd);
    if (!seg) return [];
    const at = (t: number) => {
      const runs = 10 ** (Math.log10(seg[0]) + t * (Math.log10(seg[1]) - Math.log10(seg[0])));
      return { x: x(runs), y: y(k * runs) };
    };
    const a = at(0);
    const b = at(1);
    const angle = Math.atan2(b.y - a.y, b.x - a.x);
    const length = perRunLabel(k).length * 5.4;
    for (const t of [0.88, 0.72, 0.56, 0.4, 0.24]) {
      const end = at(t);
      const start = { x: end.x - Math.cos(angle) * length, y: end.y - Math.sin(angle) * length };
      if (start.x < plot.x0 || start.y > plot.y1) continue;
      const clear = points.every((p) => !nearStrip(p.cx, p.cy, start, end));
      if (clear) return [{ k, x: end.x, y: end.y, angle: (angle * 180) / Math.PI, start, end }];
    }
    return [];
  });
  const labels = placeLabels(
    points.filter(({ o }) => o.rank <= LABELLED || o.row.org_id === activeId || isCostJump(o)),
    points,
    plot.x1,
    guideLabels
  );
  const active = points.find((p) => p.o.row.org_id === activeId);

  return (
    <div
      ref={ref}
      className='relative w-full'
      style={{ height: heightFor(width || 600) }}
      data-testid='admin-observability-map'
    >
      {width > 0 ? (
        <svg
          width={width}
          height={HEIGHT}
          aria-label='Cost to serve against agent runs, one dot per organization'
          className='overflow-visible text-[10px]'
        >
          <defs>
            <clipPath id='admin-observability-map-plot'>
              <rect x={plot.x0} y={plot.y0} width={plot.x1 - plot.x0} height={plot.y1 - plot.y0} />
            </clipPath>
          </defs>

          {/* The two gutters: no runs, and no priced cost. */}
          <rect
            x={M.left}
            y={plot.y0}
            width={GUTTER - 4}
            height={plot.y1 - plot.y0 + GUTTER}
            className='fill-muted/50'
          />
          <rect
            x={M.left}
            y={plot.y1 + 4}
            width={plot.x1 - M.left}
            height={GUTTER - 4}
            className='fill-muted/50'
          />

          {decadeTicks(yd).map((t) => (
            <g key={`y${t}`}>
              <line x1={plot.x0} x2={plot.x1} y1={y(t)} y2={y(t)} className='stroke-border/60' />
              <text
                x={M.left - 6}
                y={y(t) + 3}
                textAnchor='end'
                className='fill-muted-foreground tabular-nums'
              >
                {axisUsd(t)}
              </text>
            </g>
          ))}
          <text x={M.left - 6} y={gutterY + 3} textAnchor='end' className='fill-muted-foreground'>
            $0
          </text>

          {decadeTicks(xd).map((t) => (
            <g key={`x${t}`}>
              <line x1={x(t)} x2={x(t)} y1={plot.y0} y2={plot.y1} className='stroke-border/60' />
              <text
                x={x(t)}
                y={HEIGHT - M.bottom + 14}
                textAnchor='middle'
                className='fill-muted-foreground tabular-nums'
              >
                {axisCount(t)}
              </text>
            </g>
          ))}
          <text
            x={gutterX}
            y={HEIGHT - M.bottom + 14}
            textAnchor='middle'
            className='fill-muted-foreground'
          >
            0
          </text>
          <text x={plot.x1} y={HEIGHT - 2} textAnchor='end' className='fill-muted-foreground'>
            agent runs, log scale
          </text>

          <g clipPath='url(#admin-observability-map-plot)'>
            {guides.map((k) => (
              <line
                key={k}
                x1={x(xd[0])}
                y1={y(k * xd[0])}
                x2={x(xd[1])}
                y2={y(k * xd[1])}
                strokeDasharray='3 3'
                className='stroke-muted-foreground/40'
              />
            ))}
          </g>
          {guideLabels.map((g) => (
            <text
              key={`label${g.k}`}
              x={g.x}
              y={g.y - 4}
              textAnchor='end'
              transform={`rotate(${g.angle} ${g.x} ${g.y})`}
              className='fill-muted-foreground stroke-card [paint-order:stroke]'
              strokeWidth={3}
            >
              {perRunLabel(g.k)}
            </text>
          ))}

          {points.map(({ o, cx, cy }) => {
            const id = o.row.org_id;
            const on = id === activeId;
            const kept = matches(o);
            const failing = reliabilityTone(o.row.failure_rate) === "danger";
            return (
              <Link
                key={id}
                to={hrefFor(id)}
                aria-label={`${o.row.org_name}: ${money(o.row.cost_to_serve_usd)} to serve, ${compact(o.row.run_count)} runs`}
                onMouseEnter={() => onActive(id)}
                onMouseLeave={() => onActive(null)}
                onFocus={() => onActive(id)}
                onBlur={() => onActive(null)}
                className={cn("outline-none", !kept && "opacity-15")}
                data-testid='admin-observability-map-dot'
              >
                {/* A generous hit target: a 4px dot is hard to hover. */}
                <circle cx={cx} cy={cy} r={9} className='fill-transparent' />
                {failing ? (
                  <circle
                    cx={cx}
                    cy={cy}
                    r={on ? 9 : 7}
                    className='fill-none stroke-destructive'
                    strokeWidth={1.5}
                  />
                ) : null}
                <circle
                  cx={cx}
                  cy={cy}
                  r={on ? 6 : 4}
                  className={cn(
                    "transition-[r,opacity] duration-150",
                    isCostJump(o) ? "fill-warning" : "fill-foreground/75",
                    on && !isCostJump(o) && "fill-foreground"
                  )}
                />
                {on ? (
                  <circle cx={cx} cy={cy} r={10} className='fill-none stroke-foreground/40' />
                ) : null}
              </Link>
            );
          })}

          {labels.map(({ o, lx, ly, anchor }) => (
            <text
              key={`n${o.row.org_id}`}
              x={lx}
              y={ly}
              textAnchor={anchor}
              className={cn(
                "pointer-events-none select-none",
                o.row.org_id === activeId
                  ? "fill-foreground font-semibold"
                  : "fill-foreground/80 font-medium",
                !matches(o) && "opacity-20"
              )}
            >
              {o.row.org_name}
            </text>
          ))}
        </svg>
      ) : null}

      {active && width > 0 ? (
        <MapTooltip o={active.o} x={active.cx} y={active.cy} width={width} />
      ) : null}
    </div>
  );
};

/** A run axis spans at least 1–100, so a fleet of tiny orgs still has room. */
function widen([lo, hi]: LogDomain): LogDomain {
  return [Math.min(lo, 1), Math.max(hi, 100)];
}

/**
 * The run range over which the guide `cost = k × runs` is inside the plot,
 * or `null` when the diagonal never crosses it.
 */
function visibleSegment(
  k: number,
  [xlo, xhi]: LogDomain,
  [ylo, yhi]: LogDomain
): [number, number] | null {
  const from = Math.max(xlo, ylo / k);
  const to = Math.min(xhi, yhi / k);
  return from < to ? [from, to] : null;
}

type Box = { x0: number; x1: number; y0: number; y1: number };

/**
 * Whether a dot at (px, py) sits under a label set along the segment from
 * `a` to `b` — within its length, and on or just above the line, where the
 * text is drawn.
 */
function nearStrip(
  px: number,
  py: number,
  a: { x: number; y: number },
  b: { x: number; y: number }
): boolean {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const len = Math.hypot(dx, dy) || 1;
  const along = ((px - a.x) * dx + (py - a.y) * dy) / len;
  // Signed distance; positive is above the line on screen, where the text sits.
  const across = ((px - a.x) * dy - (py - a.y) * dx) / len;
  return along > -6 && along < len + 6 && across > -6 && across < 16;
}
const overlaps = (a: Box, b: Box) => a.x0 < b.x1 && a.x1 > b.x0 && a.y0 < b.y1 && a.y1 > b.y0;
type Point = { o: RankedOrg; cx: number; cy: number };
type Label = { o: RankedOrg; lx: number; ly: number; anchor: "start" | "end" };

/**
 * Name the costliest orgs without overprinting. Labels are placed in rank
 * order, right of the dot or else left of it, and a label that would cover
 * another label or another org's dot is skipped — the ranked list beside the
 * map names everyone, so a missing label loses nothing, while a label sitting
 * on the wrong dot says something false.
 */
function placeLabels(
  wanted: Point[],
  all: Point[],
  rightEdge: number,
  guides: { start: { x: number; y: number }; end: { x: number; y: number } }[] = []
): Label[] {
  const placed: Box[] = [];
  // A guide's label is a rotated strip, so test the box's corners and centre
  // against the strip itself rather than against its (much larger) bounds.
  const onGuide = (b: Box) =>
    guides.some((g) =>
      [
        [b.x0, b.y0],
        [b.x1, b.y0],
        [b.x0, b.y1],
        [b.x1, b.y1],
        [(b.x0 + b.x1) / 2, (b.y0 + b.y1) / 2]
      ].some(([px, py]) => nearStrip(px, py, g.start, g.end))
    );
  const out: Label[] = [];
  for (const p of [...wanted].sort((a, b) => a.o.rank - b.o.rank)) {
    const w = p.o.row.org_name.length * 5.6;
    const dots = all
      .filter((q) => q.o.row.org_id !== p.o.row.org_id)
      .map((q) => ({ x0: q.cx - 4, x1: q.cx + 4, y0: q.cy - 4, y1: q.cy + 4 }));
    const sides: Label["anchor"][] = p.cx + 8 + w > rightEdge ? ["end"] : ["start", "end"];
    for (const anchor of sides) {
      const x0 = anchor === "start" ? p.cx + 8 : p.cx - 8 - w;
      const box = { x0, x1: x0 + w, y0: p.cy - 7, y1: p.cy + 4 };
      if (
        x0 < 0 ||
        placed.some((b) => overlaps(box, b)) ||
        dots.some((d) => overlaps(box, d)) ||
        onGuide(box)
      ) {
        continue;
      }
      placed.push(box);
      out.push({ o: p.o, lx: anchor === "start" ? p.cx + 8 : p.cx - 8, ly: p.cy + 3.5, anchor });
      break;
    }
  }
  return out;
}

const MapTooltip = ({ o, x, y, width }: { o: RankedOrg; x: number; y: number; width: number }) => {
  const r = o.row;
  const perRun = r.run_count > 0 ? r.cost_to_serve_usd / r.run_count : null;
  // Keep the card inside the panel: anchor it left or right of the dot.
  const right = x > width - 200;
  return (
    <div
      role='tooltip'
      className='pointer-events-none absolute z-10 w-48 border border-border/60 bg-popover px-2.5 py-2 text-popover-foreground text-xs shadow-md'
      style={{
        left: right ? undefined : x + 14,
        right: right ? width - x + 14 : undefined,
        top: Math.max(0, y - 30)
      }}
    >
      <div className='truncate font-medium'>{r.org_name}</div>
      <dl className='mt-1 grid grid-cols-[1fr_auto] gap-x-3 gap-y-0.5 tabular-nums'>
        <dt className='text-muted-foreground'>Cost to serve</dt>
        <dd className='text-right font-medium'>{money(r.cost_to_serve_usd)}</dd>
        <dt className='text-muted-foreground'>Agent runs</dt>
        <dd className='text-right'>{compact(r.run_count)}</dd>
        <dt className='text-muted-foreground'>Per run</dt>
        <dd className='text-right'>{money(perRun)}</dd>
        {o.versus != null ? (
          <>
            <dt className='text-muted-foreground'>vs prior</dt>
            <dd className='text-right'>{versusShort(o.versus)}</dd>
          </>
        ) : null}
      </dl>
    </div>
  );
};
