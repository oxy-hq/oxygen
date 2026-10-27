import { StatGrid, StatTile } from "@/pages/ide/Experiments/components/glance/StatTile";
import { type Tone, ToneBadge } from "@/pages/ide/Experiments/components/glance/tone";
import { formatInstant } from "@/pages/ide/Experiments/dates";
import { complianceLabel, formatShare, leakLabel } from "@/pages/ide/Experiments/format";
import type { GuardrailState, RunDetail, RunState } from "@/types/experiment";
import { flipTotal } from "../operations";
import EstimateBadge from "./EstimateBadge";

const STATE: Partial<Record<RunState, { tone: Tone; label: string }>> = {
  registered: { tone: "info", label: "Registered" },
  live: { tone: "success", label: "Live" },
  closing: { tone: "warning", label: "Closing" }
};

function flipsTone(f: RunDetail["flips"], total: number): Tone | undefined {
  if (f.failed > 0) return "danger";
  if (f.unconfirmed > 0) return "warning";
  if (total > 0 && f.confirmed === total) return "success";
  return undefined;
}

function guardrailsTile(guardrails: RunDetail["guardrails"]): {
  value: string;
  sub: string;
  tone: Tone;
} {
  if (guardrails.length === 0) return { value: "None", sub: "none registered", tone: "neutral" };
  const n = (s: GuardrailState) => guardrails.filter((g) => g.status === s).length;
  const ok = n("ok");
  const sub = `${ok} of ${guardrails.length} OK`;
  if (n("breached") > 0) return { value: `${n("breached")} breached`, sub, tone: "danger" };
  if (n("refused") > 0) return { value: `${n("refused")} not evaluated`, sub, tone: "warning" };
  if (n("not_checked") > 0) {
    if (ok === 0) return { value: "Not checked yet", sub, tone: "neutral" };
    return { value: `${ok} OK`, sub: `${n("not_checked")} not checked yet`, tone: "neutral" };
  }
  return { value: "All OK", sub, tone: "success" };
}

function complianceTile(c: RunDetail["compliance"]) {
  if (c === null) {
    return {
      value: "—",
      sub: "No treated flip decided yet",
      tone: undefined
    };
  }
  const label = complianceLabel(c);
  return {
    value: label.measured ? label.text : "Not measured",
    sub: `${c.below_floor ? "Below the floor · " : ""}Leaked to control: ${leakLabel(c)}`,
    tone: c.below_floor ? ("warning" as const) : undefined
  };
}

/**
 * The run at a glance: its state, the key numbers as tiles, and the estimate's
 * standing. Operations only — there is no effect figure anywhere in this card.
 */
export default function RunSummary({ run }: { run: RunDetail }) {
  const { flips, coverage } = run;
  const state = STATE[run.state] ?? { tone: "neutral" as const, label: run.state };
  const total = flipTotal(flips);
  const guard = guardrailsTile(run.guardrails);
  const comp = complianceTile(run.compliance);
  const coverageTone: Tone =
    coverage.last_window === null
      ? "neutral"
      : coverage.last_window >= coverage.floor
        ? "success"
        : "warning";
  return (
    <section className='flex flex-col gap-3 rounded-lg border p-4' data-testid='experiment-summary'>
      <div className='flex flex-wrap items-center gap-2'>
        <ToneBadge tone={state.tone} testId='experiment-run-state'>
          {state.label}
        </ToneBadge>
        <EstimateBadge estimate={run.estimate} />
      </div>
      <StatGrid>
        <StatTile
          id='flips'
          label='Flips confirmed'
          value={`${flips.confirmed} of ${total}`}
          sub={`${flips.sent} sent · ${flips.failed} failed`}
          tone={flipsTone(flips, total)}
          hint='Switches the platform has sent to your system and had confirmed, out of all switches owed so far.'
        />
        <StatTile
          id='compliance'
          testId='experiment-compliance'
          label='Treated compliance'
          value={comp.value}
          sub={comp.sub}
          tone={comp.tone}
          hint='Share of treated units where the change actually landed. Below the floor, the result would measure the attempt, not the lever.'
        />
        <StatTile
          testId='experiment-coverage'
          id='coverage'
          label='Data coverage'
          value={formatShare(coverage.last_window)}
          sub={`floor ${formatShare(coverage.floor)}`}
          tone={coverageTone}
          hint="Share of the last window's days that have data."
        />
        <StatTile
          id='next-switch'
          testId='experiment-next-switch'
          label='Next switch'
          value={run.next_switch_on ?? "None pending"}
          hint='The next date a lever is switched.'
        />
        <StatTile
          id='next-check'
          label='Next check'
          value={formatInstant(run.next_check_at)}
          sub={`last ${run.checked_at ? formatInstant(run.checked_at) : "not yet run"}`}
          hint='When the platform next reads the data and looks at guardrails.'
        />
        <StatTile
          id='guardrails'
          label='Guardrails'
          value={guard.value}
          sub={guard.sub}
          tone={guard.tone}
          hint='Safety limits watched on every unit. One breach aborts the run.'
        />
      </StatGrid>
      <p className='text-muted-foreground text-xs' data-testid='experiment-pinned-revision'>
        Registered on {formatInstant(run.registered_at)}.
      </p>
    </section>
  );
}
