// Renderer for the analytics pipeline's `chart_rendered` blocks — the same
// payload the main web-app charts. Uses the host app's ECharts (optional
// peer, dynamic import) for interactive charts matching the web-app, and
// falls back to a dependency-free SVG render when echarts isn't installed.
// Payload shape (see web-app AnalyticsDisplayBlock / ChartConfig):
//   { config: { chart_type, x, y, series, name, value, title, … },
//     columns: string[], rows: unknown[][] }

import type { ReactNode } from "react";
import * as React from "react";
import { cx } from "./cx";

export interface ChartBlockConfig {
  chart_type: "line_chart" | "bar_chart" | "pie_chart" | "table";
  x?: string;
  y?: string;
  series?: string;
  name?: string;
  value?: string;
  title?: string;
  x_axis_label?: string;
  y_axis_label?: string;
}

export interface ChartBlock {
  config: ChartBlockConfig;
  columns: string[];
  rows: unknown[][];
}

const PALETTE = ["#3b82f6", "#22c55e", "#eab308", "#a855f7", "#06b6d4", "#f97316"];

const compact = new Intl.NumberFormat("en", { notation: "compact", maximumFractionDigits: 1 });
const toNum = (v: unknown): number => {
  const n = typeof v === "number" ? v : Number(v);
  return Number.isFinite(n) ? n : 0;
};
const toStr = (v: unknown): string => {
  if (v === null || v === undefined) return "";
  if (typeof v === "string") return v;
  if (typeof v === "number" || typeof v === "boolean") return String(v);
  // A struct or array cell: its JSON names it, "[object Object]" names nothing.
  return JSON.stringify(v);
};

/** "sales_daily__total_net_sales" → "Total Net Sales" — a friendly series
 *  label instead of the raw semantic column id. */
export function humanizeCol(name: string | undefined): string {
  const leaf = (name ?? "").split("__").pop() ?? "";
  return leaf
    .replace(/_/g, " ")
    .trim()
    .replace(/\b\w/g, (c) => c.toUpperCase());
}

interface Pivoted {
  categories: string[];
  series: Array<{ name: string; values: number[] }>;
}

/** Pivot rows into per-series value arrays aligned on x categories. */
export function pivotChart(block: ChartBlock): Pivoted {
  const { config, columns, rows } = block;
  const xi = config.x ? columns.indexOf(config.x) : 0;
  const yi = config.y ? columns.indexOf(config.y) : columns.length > 1 ? 1 : 0;
  const si = config.series ? columns.indexOf(config.series) : -1;

  const categories: string[] = [];
  const catIndex = new Map<string, number>();
  for (const row of rows) {
    const cat = toStr(row[xi]);
    if (!catIndex.has(cat)) {
      catIndex.set(cat, categories.length);
      categories.push(cat);
    }
  }

  if (si === -1) {
    const values = new Array<number>(categories.length).fill(0);
    for (const row of rows) {
      const idx = catIndex.get(toStr(row[xi]));
      if (idx !== undefined) values[idx] = toNum(row[yi]);
    }
    return { categories, series: [{ name: config.y ?? "value", values }] };
  }

  const seriesIndex = new Map<string, number[]>();
  for (const row of rows) {
    const name = toStr(row[si]);
    let values = seriesIndex.get(name);
    if (!values) {
      values = new Array<number>(categories.length).fill(0);
      seriesIndex.set(name, values);
    }
    const idx = catIndex.get(toStr(row[xi]));
    if (idx !== undefined) values[idx] = toNum(row[yi]);
  }
  return {
    categories,
    series: [...seriesIndex.entries()].map(([name, values]) => ({ name, values }))
  };
}

/** A "nice" axis ceiling so tick labels land on round numbers. */
export function niceCeil(max: number): number {
  if (max <= 0) return 1;
  const pow = 10 ** Math.floor(Math.log10(max));
  const unit = max / pow;
  const nice = unit <= 1 ? 1 : unit <= 2 ? 2 : unit <= 5 ? 5 : 10;
  return nice * pow;
}

/** Axis domain `[lo, hi]` that always includes zero, with nice round
 *  bounds. Signed measures (profit, net change) must render below a zero
 *  baseline instead of being clamped to an invisible 0..max axis. */
export function axisDomain(values: number[]): { lo: number; hi: number } {
  const rawMax = Math.max(...values, 0);
  const rawMin = Math.min(...values, 0);
  const hi = rawMax > 0 ? niceCeil(rawMax) : 0;
  const lo = rawMin < 0 ? -niceCeil(-rawMin) : 0;
  // Degenerate all-zero data: keep a unit span so ticks still render.
  return hi === lo ? { lo: 0, hi: 1 } : { lo, hi };
}

const W = 360;
const H = 190;
const M = { top: 8, right: 8, bottom: 34, left: 42 };
const IW = W - M.left - M.right;
const IH = H - M.top - M.bottom;
const TICKS = 4;

function truncate(label: string, max: number): string {
  return label.length > max ? `${label.slice(0, max - 1)}…` : label;
}

/** Map a value to its y pixel within the plot area for domain `[lo, hi]`. */
const yFor = (v: number, lo: number, hi: number): number =>
  M.top + IH * (1 - (v - lo) / (hi - lo || 1));

function Axes({ lo, hi }: { lo: number; hi: number }) {
  const span = hi - lo || 1;
  return (
    <g className='oxy-chart__grid'>
      {Array.from({ length: TICKS + 1 }, (_, i) => {
        const y = M.top + (IH * i) / TICKS;
        const value = hi - (span * i) / TICKS;
        return (
          <g key={i}>
            <line x1={M.left} x2={M.left + IW} y1={y} y2={y} />
            <text x={M.left - 5} y={y + 3} textAnchor='end' className='oxy-chart__tick'>
              {compact.format(value)}
            </text>
          </g>
        );
      })}
    </g>
  );
}

function XLabels({ categories }: { categories: string[] }) {
  const slot = IW / categories.length;
  const maxChars = Math.max(4, Math.floor(slot / 5.5));
  return (
    <g>
      {categories.map((cat, i) => (
        <text
          key={cat}
          x={M.left + slot * (i + 0.5)}
          y={H - M.bottom + 12}
          textAnchor='middle'
          className='oxy-chart__tick'
        >
          <title>{cat}</title>
          {truncate(cat, maxChars)}
        </text>
      ))}
    </g>
  );
}

function BarChart({ data }: { data: Pivoted }) {
  const { lo, hi } = axisDomain(data.series.flatMap((s) => s.values));
  const zeroY = yFor(0, lo, hi);
  const slot = IW / data.categories.length;
  const group = slot * 0.7;
  const barW = group / data.series.length;
  return (
    <svg viewBox={`0 0 ${W} ${H}`} className='oxy-chart__svg' role='img'>
      <Axes lo={lo} hi={hi} />
      {data.series.map((s, si) =>
        s.values.map((v, ci) => {
          // Bars grow from the zero baseline: up for positive values, down
          // for negative — so signed measures stay visible.
          const vy = yFor(v, lo, hi);
          return (
            <rect
              key={`${s.name}-${data.categories[ci]}`}
              x={M.left + slot * ci + (slot - group) / 2 + barW * si}
              y={Math.min(zeroY, vy)}
              width={Math.max(1, barW - 1)}
              height={Math.max(0, Math.abs(vy - zeroY))}
              rx={1.5}
              fill={PALETTE[si % PALETTE.length]}
            >
              <title>{`${data.categories[ci]}${data.series.length > 1 ? ` · ${s.name}` : ""}: ${compact.format(v)}`}</title>
            </rect>
          );
        })
      )}
      {lo < 0 && (
        <line className='oxy-chart__grid-zero' x1={M.left} x2={M.left + IW} y1={zeroY} y2={zeroY} />
      )}
      <XLabels categories={data.categories} />
    </svg>
  );
}

function LineChart({ data }: { data: Pivoted }) {
  const { lo, hi } = axisDomain(data.series.flatMap((s) => s.values));
  const zeroY = yFor(0, lo, hi);
  const slot = IW / Math.max(1, data.categories.length - 1 || 1);
  const px = (ci: number) => (data.categories.length === 1 ? M.left + IW / 2 : M.left + slot * ci);
  const py = (v: number) => yFor(v, lo, hi);
  return (
    <svg viewBox={`0 0 ${W} ${H}`} className='oxy-chart__svg' role='img'>
      <Axes lo={lo} hi={hi} />
      {lo < 0 && (
        <line className='oxy-chart__grid-zero' x1={M.left} x2={M.left + IW} y1={zeroY} y2={zeroY} />
      )}
      {data.series.map((s, si) => (
        <g key={s.name}>
          <polyline
            points={s.values.map((v, ci) => `${px(ci)},${py(v)}`).join(" ")}
            fill='none'
            stroke={PALETTE[si % PALETTE.length]}
            strokeWidth={1.8}
          />
          {s.values.map((v, ci) => (
            <circle
              key={data.categories[ci]}
              cx={px(ci)}
              cy={py(v)}
              r={2.2}
              fill={PALETTE[si % PALETTE.length]}
            >
              <title>{`${data.categories[ci]}${data.series.length > 1 ? ` · ${s.name}` : ""}: ${compact.format(v)}`}</title>
            </circle>
          ))}
        </g>
      ))}
      <XLabels categories={data.categories} />
    </svg>
  );
}

function PieChart({ block }: { block: ChartBlock }) {
  const { config, columns, rows } = block;
  const ni = config.name ? columns.indexOf(config.name) : config.x ? columns.indexOf(config.x) : 0;
  const vi = config.value
    ? columns.indexOf(config.value)
    : config.y
      ? columns.indexOf(config.y)
      : 1;
  const slices = rows.map((r) => ({ name: toStr(r[ni]), value: Math.max(0, toNum(r[vi])) }));
  const total = slices.reduce((sum, s) => sum + s.value, 0) || 1;

  const cxp = W / 2;
  const cyp = H / 2;
  const r = Math.min(W, H) / 2 - 14;
  let angle = -Math.PI / 2;
  const paths = slices.map((s, i) => {
    const sweep = (s.value / total) * Math.PI * 2;
    const title = `${s.name}: ${compact.format(s.value)} (${Math.round((s.value / total) * 100)}%)`;
    const fill = PALETTE[i % PALETTE.length];
    // A slice covering the whole circle (single slice, or one category at
    // 100%) has coincident arc endpoints, which SVG drops — collapsing the
    // path to a zero-area sliver that renders blank. Draw a full circle
    // instead.
    if (sweep >= Math.PI * 2 - 1e-6) {
      angle += sweep;
      return (
        <circle
          key={s.name}
          cx={cxp}
          cy={cyp}
          r={r}
          fill={fill}
          stroke='var(--oxy-shell-background, #fff)'
          strokeWidth={1}
        >
          <title>{title}</title>
        </circle>
      );
    }
    const x1 = cxp + r * Math.cos(angle);
    const y1 = cyp + r * Math.sin(angle);
    angle += sweep;
    const x2 = cxp + r * Math.cos(angle);
    const y2 = cyp + r * Math.sin(angle);
    const large = sweep > Math.PI ? 1 : 0;
    return (
      <path
        key={s.name}
        d={`M ${cxp} ${cyp} L ${x1} ${y1} A ${r} ${r} 0 ${large} 1 ${x2} ${y2} Z`}
        fill={fill}
        stroke='var(--oxy-shell-background, #fff)'
        strokeWidth={1}
      >
        <title>{title}</title>
      </path>
    );
  });
  return (
    <svg viewBox={`0 0 ${W} ${H}`} className='oxy-chart__svg' role='img'>
      {paths}
    </svg>
  );
}

const TABLE_MAX_ROWS = 10;

function TableChart({ block }: { block: ChartBlock }) {
  const visible = block.rows.slice(0, TABLE_MAX_ROWS);
  return (
    <div className='oxy-chart__table-wrap'>
      <table className='oxy-chart__table'>
        <thead>
          <tr>
            {block.columns.map((c) => (
              <th key={c}>{c}</th>
            ))}
          </tr>
        </thead>
        <tbody>
          {visible.map((row, i) => (
            <tr key={row.map(toStr).join("|") || i}>
              {block.columns.map((c, j) => (
                <td key={c}>{toStr(row[j])}</td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
      {block.rows.length > TABLE_MAX_ROWS && (
        <div className='oxy-chart__note'>+{block.rows.length - TABLE_MAX_ROWS} more rows</div>
      )}
    </div>
  );
}

/** Dependency-free SVG rendering — the fallback when the host app has no
 *  `echarts`. */
function SvgChart({ block }: { block: ChartBlock }) {
  const data =
    block.config.chart_type === "bar_chart" || block.config.chart_type === "line_chart"
      ? pivotChart(block)
      : null;
  let body: ReactNode;
  switch (block.config.chart_type) {
    case "bar_chart":
      body = data && <BarChart data={data} />;
      break;
    case "line_chart":
      body = data && <LineChart data={data} />;
      break;
    case "pie_chart":
      body = <PieChart block={block} />;
      break;
    default:
      body = <TableChart block={block} />;
  }
  const multiSeries = data && data.series.length > 1;
  return (
    <>
      {body}
      {multiSeries && (
        <div className='oxy-chart__legend'>
          {data.series.map((s, i) => (
            <span key={s.name} className='oxy-chart__legend-item'>
              <span
                className='oxy-chart__legend-dot'
                style={{ background: PALETTE[i % PALETTE.length] }}
              />
              {s.name}
            </span>
          ))}
        </div>
      )}
    </>
  );
}

/** Read the shell's design tokens off an element so the ECharts chart
 *  matches the surrounding light/dark theme. */
function tokenColors(el: HTMLElement) {
  const cs = getComputedStyle(el);
  const pick = (name: string, fallback: string) => cs.getPropertyValue(name).trim() || fallback;
  return {
    fg: pick("--oxy-shell-foreground", "#0a0a0a"),
    muted: pick("--oxy-shell-muted-fg", "#71717a"),
    border: pick("--oxy-shell-border", "#d4d4d8"),
    bg: pick("--oxy-shell-background", "#ffffff")
  };
}

/** Build the ECharts option for a `chart_rendered` block — the analytics
 *  pipeline's `{chart_type, x, y, series, …}` config, the same shape the
 *  main web-app charts. Interactive: axis/item tooltips, legend, hover. */
function buildEchartsOption(block: ChartBlock, el: HTMLElement): Record<string, unknown> {
  const { fg, muted, border } = tokenColors(el);
  const axisLabel = { color: muted, fontSize: 10 };
  const ct = block.config.chart_type;

  if (ct === "pie_chart") {
    const { config, columns, rows } = block;
    const ni = config.name
      ? columns.indexOf(config.name)
      : config.x
        ? columns.indexOf(config.x)
        : 0;
    const vi = config.value
      ? columns.indexOf(config.value)
      : config.y
        ? columns.indexOf(config.y)
        : 1;
    const data = rows.map((r) => ({ name: toStr(r[ni]), value: Math.max(0, toNum(r[vi])) }));
    return {
      color: PALETTE,
      tooltip: { trigger: "item", formatter: "{b}: {c} ({d}%)" },
      legend: { bottom: 0, textStyle: { color: muted, fontSize: 10 }, type: "scroll" },
      series: [
        {
          type: "pie",
          radius: ["38%", "66%"],
          center: ["50%", "44%"],
          data,
          label: { color: fg, fontSize: 10 },
          labelLine: { lineStyle: { color: border } }
        }
      ]
    };
  }

  const pivot = pivotChart(block);
  const multi = pivot.series.length > 1;
  // Friendly single-series label: the axis label, else the humanized y col
  // (never the raw "table__column" id that shows in the tooltip/legend).
  const singleName = block.config.y_axis_label || humanizeCol(block.config.y) || "Value";
  // Long / many categories overlap on a horizontal axis → show every label
  // rotated + truncated so no bar is left unlabeled.
  const longLabels = pivot.categories.length > 4 || pivot.categories.some((c) => c.length > 10);
  return {
    color: PALETTE,
    tooltip: {
      trigger: "axis",
      valueFormatter: (v: unknown) => (typeof v === "number" ? compact.format(v) : String(v))
    },
    grid: { left: 6, right: 12, top: multi ? 30 : 12, bottom: 4, containLabel: true },
    legend: multi
      ? { top: 0, textStyle: { color: muted, fontSize: 10 }, type: "scroll" }
      : undefined,
    xAxis: {
      type: "category",
      data: pivot.categories,
      name: block.config.x_axis_label,
      nameLocation: "middle",
      nameGap: longLabels ? 42 : 24,
      nameTextStyle: { color: muted, fontSize: 10 },
      axisLabel: {
        ...axisLabel,
        interval: 0,
        rotate: longLabels ? 30 : 0,
        formatter: (v: string) => (v.length > 18 ? `${v.slice(0, 17)}…` : v)
      },
      axisTick: { show: false },
      axisLine: { lineStyle: { color: border } }
    },
    yAxis: {
      type: "value",
      name: block.config.y_axis_label,
      nameTextStyle: { color: muted, fontSize: 10 },
      axisLabel: { ...axisLabel, formatter: (v: number) => compact.format(v) },
      splitLine: { lineStyle: { color: border, opacity: 0.5 } }
    },
    series: pivot.series.map((s, i) => ({
      name: multi ? s.name : singleName,
      type: ct === "bar_chart" ? "bar" : "line",
      data: s.values,
      itemStyle: { color: PALETTE[i % PALETTE.length] },
      ...(ct === "line_chart" ? { lineStyle: { width: 2 }, symbolSize: 5, smooth: false } : {}),
      barMaxWidth: 32
    }))
  };
}

export interface AnswerChartProps {
  block: ChartBlock;
  className?: string;
}

/** Renders one `chart_rendered` block. Uses ECharts (interactive, matching
 *  the main web-app) when the host app has it installed; otherwise falls
 *  back to a dependency-free SVG render. Table type always uses SVG. */
export function AnswerChart({ block, className }: AnswerChartProps) {
  const ref = React.useRef<HTMLDivElement>(null);
  // "pending" while we try ECharts; "svg" once we know it isn't available
  // (or for the table type, which ECharts doesn't draw).
  const [engine, setEngine] = React.useState<"pending" | "svg">(
    block.config.chart_type === "table" ? "svg" : "pending"
  );

  React.useEffect(() => {
    if (engine === "svg") return;
    let disposed = false;
    let chart: { setOption: (o: unknown) => void; resize: () => void; dispose: () => void } | null =
      null;
    let ro: ResizeObserver | null = null;
    // `@vite-ignore` keeps the specifier out of the consumer's static build
    // graph: echarts is an OPTIONAL peer, so a bundle that doesn't install it
    // must still `vite build`. Without this, Rollup tries to resolve "echarts"
    // at build time and aborts — the runtime `.catch()` SVG fallback below
    // never gets a chance to run. Ignored here → resolved at runtime → the
    // catch fires and we fall back to SVG when echarts is absent.
    import(/* @vite-ignore */ "echarts")
      .then((echarts) => {
        const el = ref.current;
        if (disposed || !el) return;
        chart = echarts.init(el, undefined, { renderer: "canvas" });
        chart.setOption(buildEchartsOption(block, el));
        ro = new ResizeObserver(() => chart?.resize());
        ro.observe(el);
      })
      .catch(() => {
        // echarts not installed in the host app → SVG fallback.
        if (!disposed) setEngine("svg");
      });
    return () => {
      disposed = true;
      ro?.disconnect();
      chart?.dispose();
    };
  }, [block, engine]);

  return (
    <figure className={cx("oxy-chart", className)} data-testid='askdock-chart'>
      {block.config.title && (
        <figcaption className='oxy-chart__title'>{block.config.title}</figcaption>
      )}
      {engine === "svg" ? (
        <SvgChart block={block} />
      ) : (
        <div ref={ref} className='oxy-chart__echarts' style={{ width: "100%", height: 240 }} />
      )}
    </figure>
  );
}
