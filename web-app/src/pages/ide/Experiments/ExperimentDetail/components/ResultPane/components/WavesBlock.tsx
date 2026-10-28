import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from "@/components/ui/shadcn/table";
import { ToneBadge } from "@/pages/ide/Experiments/components/glance/tone";
import { NOT_AVAILABLE, reportFormat } from "@/pages/ide/Experiments/format";
import type { ReportView, WaveView } from "@/types/experiment";
import Section from "../../Section";
import HowCalculated from "./HowCalculated";

function verdict(w: WaveView) {
  if (w.refusal) return <ToneBadge tone='neutral'>Not compared</ToneBadge>;
  return w.significant ? (
    <ToneBadge tone='success'>Clear on its own</ToneBadge>
  ) : (
    <ToneBadge tone='neutral'>No clear effect</ToneBadge>
  );
}

/**
 * Each wave's own effect beside the pooled one. The pooled headline is a weighted
 * mean over waves, so a wave that clearly moved can be averaged away by waves
 * that did not; this shows the parts. Only rendered when there are two or more.
 */
export default function WavesBlock({ report }: { report: ReportView }) {
  const waves = report.waves ?? [];
  if (waves.length < 2) return null;
  const fmt = reportFormat(report);
  return (
    <Section title='Each wave on its own' testId='experiment-report-waves'>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Switched on</TableHead>
            <TableHead>Treated</TableHead>
            <TableHead>Compared with</TableHead>
            <TableHead>Effect</TableHead>
            <TableHead>Interval</TableHead>
            <TableHead />
          </TableRow>
        </TableHeader>
        <TableBody>
          {waves.map((w, i) => (
            <TableRow key={w.switch_on ?? i} data-testid='experiment-report-wave'>
              <TableCell>{w.switch_on ?? NOT_AVAILABLE}</TableCell>
              <TableCell className='tabular-nums'>{w.n_treated}</TableCell>
              <TableCell className='tabular-nums'>{w.n_control}</TableCell>
              <TableCell className='font-medium tabular-nums'>{fmt.signed(w.estimate)}</TableCell>
              <TableCell className='tabular-nums'>
                {w.refusal ? (
                  <span className='text-muted-foreground text-xs'>{w.refusal}</span>
                ) : (
                  fmt.interval(w.interval.low, w.interval.high)
                )}
              </TableCell>
              <TableCell>{verdict(w)}</TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
      <HowCalculated>
        <p>
          Each wave is compared on its own with the units not yet switched, over the same before and
          after windows as the overall result.
        </p>
        <p>
          These intervals are wider than the overall one and are not adjusted for the number of
          waves, so one wave that looks clear is a lead to follow up, not a finding. The overall
          effect above is the answer to the question the experiment asked.
        </p>
      </HowCalculated>
    </Section>
  );
}
