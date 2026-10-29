import type { EChartsOption } from "echarts";
import { useMemo, useState } from "react";
import { Echarts } from "@/components/Echarts";
import { resolveColor } from "@/components/Echarts/resolveColor";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from "@/components/ui/shadcn/table";
import { HintIcon } from "@/pages/ide/Experiments/components/glance/Hinted";
import { type Tone, ToneBadge } from "@/pages/ide/Experiments/components/glance/tone";
import { formatAmount, NOT_AVAILABLE, reportFormat } from "@/pages/ide/Experiments/format";
import type { ReportView, SyntheticFitView } from "@/types/experiment";
import Section from "../../Section";
import HowCalculated from "./HowCalculated";

const FIT_HINT =
  "How closely the synthetic control followed this unit before the change, against how much the unit moves on its own. Close means the comparison is trustworthy; loose means part of the effect may be the fit's error.";
const RANK_HINT =
  "Each unchanged unit is also given a synthetic control, as if it had been changed. This is the share of them whose before/after gap was at least as large as this unit's: small means this unit's gap stands out.";

/** A plain word for the pre-period fit: 0 tracks perfectly, 1 is no better than a flat line. */
export function fitWord(preFit: number | null): { label: string; tone: Tone } {
  if (preFit === null) return { label: NOT_AVAILABLE, tone: "neutral" };
  if (preFit < 0.3) return { label: "Close", tone: "success" };
  if (preFit < 0.6) return { label: "Fair", tone: "warning" };
  return { label: "Loose", tone: "danger" };
}

/** `b3 70%, b9 30%`: the donors a unit's synthetic control is made of, largest first. */
export function builtFrom(f: SyntheticFitView, shown = 3): string {
  const top = f.weights
    .slice(0, shown)
    .map((w) => `${w.unit} ${Math.round(w.weight * 100)}%`)
    .join(", ");
  const rest = f.weights.length - shown;
  return rest > 0 ? `${top}, and ${rest} more` : top || NOT_AVAILABLE;
}

function chartOptions(f: SyntheticFitView): EChartsOption {
  const dates = f.series.map((p) => p.date ?? "");
  return {
    tooltip: {
      trigger: "axis",
      valueFormatter: (v) => formatAmount(typeof v === "number" ? v : null)
    },
    legend: { bottom: 0 },
    grid: { containLabel: true, left: 8, right: 16, top: 24, bottom: 40 },
    xAxis: { type: "category", data: dates, boundaryGap: false },
    yAxis: { type: "value", scale: true },
    series: [
      {
        name: f.unit,
        type: "line",
        showSymbol: false,
        data: f.series.map((p) => p.actual),
        lineStyle: { color: resolveColor("--chart-1") },
        itemStyle: { color: resolveColor("--chart-1") }
      },
      {
        name: "Synthetic control",
        type: "line",
        showSymbol: false,
        data: f.series.map((p) => p.synthetic),
        lineStyle: { color: resolveColor("--chart-2"), type: "dashed" },
        itemStyle: { color: resolveColor("--chart-2") }
      }
    ]
  };
}

/**
 * Each changed unit against its own synthetic control: how well the control
 * tracked it before the change, what it is made of, and the two lines over the
 * before and after windows. Only on a report that used a synthetic control.
 */
export default function SyntheticBlock({ report }: { report: ReportView }) {
  const s = report.synthetic;
  const [picked, setPicked] = useState(0);
  const fit = s?.fits[Math.min(picked, (s?.fits.length ?? 1) - 1)];
  const options = useMemo(() => (fit ? chartOptions(fit) : null), [fit]);
  if (!s || s.fits.length === 0 || !fit || !options) return null;
  const fmt = reportFormat(report);
  return (
    <Section
      title='Each changed unit against its synthetic control'
      testId='experiment-report-synthetic'
    >
      <div>
        <ToneBadge tone='info' testId='experiment-report-synthetic-label'>
          {s.label}
        </ToneBadge>
      </div>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Unit</TableHead>
            <TableHead>Effect</TableHead>
            <TableHead>
              <span className='inline-flex items-center gap-1'>
                Tracked before
                <HintIcon hint={FIT_HINT} testId='experiment-report-synthetic-fit-hint' />
              </span>
            </TableHead>
            <TableHead>
              <span className='inline-flex items-center gap-1'>
                Stands out
                <HintIcon hint={RANK_HINT} testId='experiment-report-synthetic-rank-hint' />
              </span>
            </TableHead>
            <TableHead>Built from</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {s.fits.map((f, i) => {
            const w = fitWord(f.pre_fit);
            return (
              <TableRow
                key={f.unit}
                data-testid='experiment-report-synthetic-unit'
                data-selected={i === picked}
                className='cursor-pointer data-[selected=true]:bg-muted/50'
                onClick={() => setPicked(i)}
              >
                <TableCell className='font-medium'>{f.unit}</TableCell>
                <TableCell className='tabular-nums'>{fmt.signed(f.effect)}</TableCell>
                <TableCell>
                  <ToneBadge tone={w.tone}>{w.label}</ToneBadge>
                </TableCell>
                <TableCell className='tabular-nums'>
                  {f.rank_p === null ? NOT_AVAILABLE : `top ${Math.round(f.rank_p * 100)}%`}
                </TableCell>
                <TableCell className='text-muted-foreground text-xs'>{builtFrom(f)}</TableCell>
              </TableRow>
            );
          })}
        </TableBody>
      </Table>
      <p className='text-muted-foreground text-sm'>
        {s.fits.length > 1 ? "Click a unit to chart it. " : ""}
        {fit.unit} and its synthetic control, before and after the change:
      </p>
      <Echarts
        options={options}
        isLoading={false}
        height='280px'
        testId='experiment-report-synthetic-chart'
      />
      <HowCalculated>
        <p>
          Nothing was randomised. For each changed unit, a synthetic control is a weighted mix of
          the {s.donors} unchanged units, with weights chosen so the mix follows the unit as closely
          as possible before the change. After the change, the gap between the unit and its mix is
          the effect; the headline averages it over the changed units.
        </p>
        <p>
          The p-value treats each unchanged unit as if it had been changed and asks how often their
          gaps are as large as the real one ({s.placebo_sets} such sets
          {s.exact ? ", every one there is" : ", drawn at random"}).
        </p>
      </HowCalculated>
    </Section>
  );
}
