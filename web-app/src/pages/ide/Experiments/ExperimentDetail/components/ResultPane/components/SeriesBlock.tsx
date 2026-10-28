import { useMemo } from "react";
import { Echarts } from "@/components/Echarts";
import { resolveColor, resolveColorWithAlpha } from "@/components/Echarts/resolveColor";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from "@/components/ui/shadcn/table";
import { formatAmount, formatSignedAmount } from "@/pages/ide/Experiments/format";
import { grainOf, periodWord } from "@/pages/ide/Experiments/grain";
import type { DailySeries } from "@/types/experiment";
import Section from "../../Section";
import { seriesChartOptions, seriesRows, seriesSummary } from "../series";

/**
 * The target measure day by day across the whole run, as sealed with the result:
 * a chart with the post windows (or a switchback's on periods) shaded and each
 * arm's 7-day average drawn over its faded daily line, and the same days as a
 * table. A missing arm mean is a dash, never 0.
 */
export default function SeriesBlock({
  series,
  retrospective = false,
  proportional = false
}: {
  series: DailySeries;
  /** Nothing was switched: the lever column shows a dash. */
  retrospective?: boolean;
  /** The headline is a % lift: say these figures are the measure's own units. */
  proportional?: boolean;
}) {
  const options = useMemo(
    () =>
      seriesChartOptions(series, {
        treated: resolveColor("--chart-1"),
        control: resolveColor("--chart-2"),
        band: resolveColorWithAlpha("--chart-1", 0.08)
      }),
    [series]
  );
  const rows = seriesRows(series, { retrospective });
  const waves = series.design === "waves";
  return (
    <Section
      title={`${periodWord(grainOf(series.grain), { capital: true })} by ${periodWord(grainOf(series.grain))}, across the whole run`}
      testId='experiment-report-series'
    >
      <p className='text-muted-foreground text-sm'>{seriesSummary(series)}</p>
      {proportional && (
        <p className='text-muted-foreground text-sm' data-testid='experiment-report-series-units'>
          These figures are the measure itself, not percentages. The % lift above compares each unit
          to its own level, so a unit selling 2 a day counts as much as one selling 200.
        </p>
      )}
      <Echarts
        options={options}
        isLoading={false}
        height='320px'
        testId='experiment-report-series-chart'
      />
      <Table
        containerClassName='mt-2 max-h-96 overflow-y-auto'
        data-testid='experiment-report-series-table'
      >
        <TableHeader>
          <TableRow>
            <TableHead>Date</TableHead>
            {waves ? (
              <>
                <TableHead className='text-right'>Treated mean</TableHead>
                <TableHead className='text-right'>Control mean</TableHead>
                <TableHead className='text-right'>Difference</TableHead>
              </>
            ) : (
              <TableHead className='text-right'>Fleet mean</TableHead>
            )}
            <TableHead>Lever</TableHead>
            <TableHead>Window</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.map((r) => (
            <TableRow key={r.date}>
              <TableCell>{r.date}</TableCell>
              {waves ? (
                <>
                  <TableCell className='text-right'>{formatAmount(r.treated)}</TableCell>
                  <TableCell className='text-right'>{formatAmount(r.control)}</TableCell>
                  <TableCell className='text-right'>{formatSignedAmount(r.difference)}</TableCell>
                </>
              ) : (
                <TableCell className='text-right'>{formatAmount(r.fleet)}</TableCell>
              )}
              <TableCell>{r.lever}</TableCell>
              <TableCell>{r.window}</TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </Section>
  );
}
