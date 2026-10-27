import { Badge } from "@/components/ui/shadcn/badge";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from "@/components/ui/shadcn/table";
import { type Tone, ToneBadge } from "@/pages/ide/Experiments/components/glance/tone";
import { NOT_AVAILABLE } from "@/pages/ide/Experiments/format";
import type { RunPeriod } from "@/types/experiment";
import HowCalculated from "../../ResultPane/components/HowCalculated";
import Section from "../../Section";
import { type PeriodStatus, periodRows } from "../operations";

const STATUS_LABEL: Record<PeriodStatus, string> = {
  upcoming: "Upcoming",
  in_progress: "In progress",
  landed: "Landed",
  needs_attention: "Needs attention"
};

const STATUS_TONE: Record<PeriodStatus, Tone> = {
  upcoming: "neutral",
  in_progress: "info",
  landed: "success",
  needs_attention: "danger"
};

/** A switchback's periods in their pairs, with how far each period's per-unit flips have got. */
export default function SwitchbackSchedule({ periods }: { periods: RunPeriod[] }) {
  const rows = periodRows(periods);
  const count: Record<PeriodStatus, number> = {
    upcoming: 0,
    in_progress: 0,
    landed: 0,
    needs_attention: 0
  };
  for (const r of rows) count[r.status] += 1;
  return (
    <Section title='Switchback schedule' testId='experiment-switchback'>
      <div className='flex flex-col gap-3'>
        <div
          className='flex flex-wrap items-center gap-2'
          data-testid='experiment-switchback-summary'
        >
          <ToneBadge tone='neutral'>
            {rows.length} period{rows.length === 1 ? "" : "s"}
          </ToneBadge>
          <ToneBadge tone={count.landed > 0 ? "success" : "neutral"}>
            {count.landed} landed
          </ToneBadge>
          {count.in_progress > 0 && (
            <ToneBadge tone='info'>{count.in_progress} in progress</ToneBadge>
          )}
          {count.needs_attention > 0 && (
            <ToneBadge tone='danger'>{count.needs_attention} need attention</ToneBadge>
          )}
          {count.upcoming > 0 && <ToneBadge tone='neutral'>{count.upcoming} upcoming</ToneBadge>}
        </div>
        <HowCalculated title='How this works'>
          <p>
            The whole fleet alternates. Each pair is one on period and one off period in a seeded
            random order, and every unit flips at each boundary; flip progress counts those per-unit
            flips.
          </p>
        </HowCalculated>
        <HowCalculated title='Show all periods'>
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Pair</TableHead>
                <TableHead>Starts</TableHead>
                <TableHead>State</TableHead>
                <TableHead>Flips</TableHead>
                <TableHead>Status</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.map((r) => (
                <TableRow
                  key={r.from}
                  data-testid={`experiment-period-${r.index + 1}`}
                  data-pair={r.pair}
                  data-state={r.state}
                  data-status={r.status}
                >
                  <TableCell>{r.pair}</TableCell>
                  <TableCell>{r.from}</TableCell>
                  <TableCell>
                    <Badge variant={r.state === "on" ? "default" : "outline"}>
                      {r.state === "on" ? "On" : "Off"}
                    </Badge>
                  </TableCell>
                  <TableCell>
                    {r.total === 0
                      ? NOT_AVAILABLE
                      : `${r.confirmed} of ${r.total} confirmed${r.failed > 0 ? `, ${r.failed} failed` : ""}`}
                  </TableCell>
                  <TableCell>
                    <ToneBadge tone={STATUS_TONE[r.status]}>{STATUS_LABEL[r.status]}</ToneBadge>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </HowCalculated>
      </div>
    </Section>
  );
}
