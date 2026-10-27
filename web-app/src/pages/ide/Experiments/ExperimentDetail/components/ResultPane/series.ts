import type { EChartsOption, MarkAreaComponentOption, SeriesOption } from "echarts";
import { formatMoney } from "@/pages/ide/Experiments/format";
import { type Grain, grainOf, periods, periodWord } from "@/pages/ide/Experiments/grain";
import type { DailySeries, SeriesPeriod, SeriesWave } from "@/types/experiment";

/** One table row. Waves fill `treated`/`control`/`difference`; a switchback fills `fleet`. */
export interface SeriesRow {
  date: string;
  treated: number | null;
  control: number | null;
  difference: number | null;
  fleet: number | null;
  lever: string;
  window: string;
}

export interface SeriesColors {
  treated: string;
  control: string;
  band: string;
}

type WavesSeries = Extract<DailySeries, { design: "waves" }>;
type SwitchbackSeries = Extract<DailySeries, { design: "switchback" }>;

/** ISO dates compare as strings; every `to` is exclusive. */
const within = (date: string, from: string, to: string) => date >= from && date < to;

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

function leverLabel(on: number, treated: number): string {
  if (on === 0) return "Off";
  if (on >= treated) return "On";
  return `${on} of ${treated} on`;
}

/** "Pre window" on a single wave; "Wave 1 post, wave 2 pre" when the waves overlap. */
function windowLabel(date: string, waves: SeriesWave[]): string {
  const parts = waves.flatMap((w, i) => {
    const which = within(date, w.pre_from, w.pre_to)
      ? "pre"
      : within(date, w.post_from, w.post_to)
        ? "post"
        : null;
    if (!which) return [];
    return [waves.length === 1 ? `${which} window` : `wave ${i + 1} ${which}`];
  });
  const text = parts.join(", ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}

/** A retrospective switched nothing: its lever column is a dash, never On or Off. */
const NEVER_SWITCHED = "—";

export function seriesRows(s: DailySeries, opts: { retrospective?: boolean } = {}): SeriesRow[] {
  const lever = (label: string) => (opts.retrospective ? NEVER_SWITCHED : label);
  if (s.design === "switchback") {
    return s.days.map((d) => ({
      date: d.date,
      treated: null,
      control: null,
      difference: null,
      fleet: d.fleet,
      lever: lever(d.on ? "On" : "Off"),
      window: d.washout ? "Washout" : ""
    }));
  }
  return s.days.map((d) => ({
    date: d.date,
    treated: d.treated,
    control: d.control,
    difference: d.treated !== null && d.control !== null ? d.treated - d.control : null,
    fleet: null,
    lever: lever(leverLabel(d.on_units, s.treated_units)),
    window: windowLabel(d.date, s.waves)
  }));
}

export function seriesSummary(s: DailySeries): string {
  const first = s.days[0]?.date;
  const last = s.days[s.days.length - 1]?.date;
  const arms =
    s.design === "waves"
      ? `${s.treated_units} treated and ${s.control_units} control units`
      : `mean over a fleet of ${plural(s.fleet_units, "unit")}`;
  const grain = grainOf(s.grain);
  const span = first
    ? `, ${first} to ${last} (${periods(s.days.length, grain)})`
    : `, no ${periodWord(grain, { plural: true })} read`;
  const cut = s.measured_until ? ` Measurement stopped before ${s.measured_until}.` : "";
  return `${s.measure} per unit-${grain}, ${arms}${span}.${cut}`;
}

const DAY_MS = 86_400_000;
const TREND_DAYS = 7;

/**
 * The trend each arm's line is averaged over: `points` periods, centred, spanning
 * `days` calendar days (so `days / 2` reaches the neighbours it should and no further).
 */
const TREND: Record<Grain, { points: number; days: number }> = {
  hour: { points: 24, days: 1 },
  day: { points: TREND_DAYS, days: TREND_DAYS },
  week: { points: 3, days: 21 },
  month: { points: 3, days: 90 }
};

/**
 * A centred rolling mean over calendar days: each day averages the values dated
 * within `days / 2` either side of it. Days the panel dropped and missing arm
 * means are skipped, so the window holds up to `days` values, fewer at the edges.
 * A day with nothing in its window is null, never 0.
 */
export function rollingMean(
  dates: string[],
  values: (number | null)[],
  days = TREND_DAYS
): (number | null)[] {
  const times = dates.map((d) => Date.parse(d));
  // Whole days either side; under a day (hour grain), half the window in hours.
  const reach = days >= 2 ? Math.floor(days / 2) * DAY_MS : (days / 2) * DAY_MS;
  return times.map((t) => {
    let sum = 0;
    let n = 0;
    times.forEach((u, j) => {
      const v = values[j];
      if (v !== null && Math.abs(u - t) <= reach) {
        sum += v;
        n += 1;
      }
    });
    return n === 0 ? null : sum / n;
  });
}

/** The first and last retained day inside `[from, to)`, or nothing when the panel kept none. */
function band(dates: string[], from: string, to: string, name: string) {
  const inside = dates.filter((d) => within(d, from, to));
  if (inside.length === 0) return [];
  return [[{ xAxis: inside[0], name }, { xAxis: inside[inside.length - 1] }]];
}

function markArea(data: unknown[], color: string): MarkAreaComponentOption {
  return {
    silent: true,
    itemStyle: { color },
    label: { position: "insideTop" },
    data: data as MarkAreaComponentOption["data"]
  };
}

/** A day's raw line, faded so the trend drawn over it reads first. */
function dailyLine(name: string, data: (number | null)[], color: string): SeriesOption {
  return {
    type: "line",
    name,
    data,
    color,
    showSymbol: false,
    lineStyle: { width: 1, opacity: 0.35 }
  };
}

/** An arm's rolling average (7 days, 3 weeks or 3 months), bold over its raw line. */
function trendLine(
  arm: string,
  dates: string[],
  values: (number | null)[],
  color: string,
  grain: Grain
): SeriesOption {
  const t = TREND[grain];
  return {
    type: "line",
    name: `${arm}, ${t.points}-${grain} average`,
    data: rollingMean(dates, values, t.days),
    color,
    showSymbol: false,
    lineStyle: { width: 3 }
  };
}

function wavesSeries(s: WavesSeries, dates: string[], c: SeriesColors): SeriesOption[] {
  const posts = s.waves.flatMap((w, i) =>
    band(dates, w.post_from, w.post_to, s.waves.length === 1 ? "Post window" : `Wave ${i + 1} post`)
  );
  const treated = s.days.map((d) => d.treated);
  const control = s.days.map((d) => d.control);
  return [
    {
      ...dailyLine(`Treated (${plural(s.treated_units, "unit")})`, treated, c.treated),
      markArea: markArea(posts, c.band)
    },
    dailyLine(`Control (${plural(s.control_units, "unit")})`, control, c.control),
    trendLine("Treated", dates, treated, c.treated, grainOf(s.grain)),
    trendLine("Control", dates, control, c.control, grainOf(s.grain))
  ];
}

function switchbackSeries(s: SwitchbackSeries, dates: string[], c: SeriesColors): SeriesOption[] {
  const ons = s.periods
    .filter((p: SeriesPeriod) => p.on)
    .flatMap((p) => band(dates, p.from, p.to, "On"));
  return [
    {
      type: "line",
      name: `Fleet mean (${plural(s.fleet_units, "unit")})`,
      data: s.days.map((d) => d.fleet),
      color: c.treated,
      showSymbol: false,
      markArea: markArea(ons, c.band)
    }
  ];
}

export function seriesChartOptions(s: DailySeries, c: SeriesColors): EChartsOption {
  const dates = s.days.map((d) => d.date);
  return {
    tooltip: {
      trigger: "axis",
      valueFormatter: (v) => formatMoney(typeof v === "number" ? v : null)
    },
    legend: { bottom: 0 },
    grid: { containLabel: true, left: 8, right: 16, top: 24, bottom: 40 },
    xAxis: { type: "category", data: dates, boundaryGap: false },
    yAxis: { type: "value", scale: true },
    series: s.design === "waves" ? wavesSeries(s, dates, c) : switchbackSeries(s, dates, c)
  };
}
