import { AlertTriangle } from "lucide-react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/shadcn/alert";
import { CheckList } from "@/pages/ide/Experiments/components/glance/CheckList";
import { ToneBadge } from "@/pages/ide/Experiments/components/glance/tone";
import type { RunDetail } from "@/types/experiment";
import Section from "../../Section";
import GroupMembership from "./GroupMembership";
import GuardrailList from "./GuardrailList";
import InterimPanel from "./InterimPanel";
import RunSummary from "./RunSummary";
import SwitchbackSchedule from "./SwitchbackSchedule";

/**
 * Operations only. Every field is read by name — the payload is never spread —
 * and `RunDetail` has no effect field, so there is nothing here to leak: the
 * registered effect stays sealed until the run reports; the interim panel is a
 * labelled peek, kept apart from the result. (`compliance.restated_mde` is a
 * per unit-day figure and is deliberately not shown here; it belongs to the report.)
 */
export default function RunBody({
  name,
  run,
  leverRevisions
}: {
  name: string;
  run: RunDetail;
  /** `RunStatus.lever_revisions`: more than one means the lever changed mid-run. */
  leverRevisions: string[];
}) {
  const { flips } = run;
  return (
    <div className='flex flex-col gap-4'>
      {run.not_advancing && (
        <Alert variant='destructive' data-testid='experiment-not-advancing'>
          <AlertTriangle />
          <AlertTitle>Not advancing</AlertTitle>
          <AlertDescription>
            This experiment's scheduler isn't running — nothing will flip until it is.
          </AlertDescription>
        </Alert>
      )}
      {run.guardrail_breach && (
        <Alert variant='destructive' data-testid='experiment-guardrail-breach'>
          <AlertTriangle />
          <AlertTitle>Guardrail breached</AlertTitle>
          <AlertDescription>
            <p className='whitespace-pre-wrap'>{run.guardrail_breach}</p>
          </AlertDescription>
        </Alert>
      )}
      {leverRevisions.length > 1 && (
        <Alert data-testid='experiment-lever-changed'>
          <AlertTitle>The lever's definition changed mid-run</AlertTitle>
          <AlertDescription>
            Flips ran against {leverRevisions.length} revisions of the on/off automations.
          </AlertDescription>
        </Alert>
      )}

      <RunSummary run={run} />
      {(run.state === "live" || run.state === "closing") && !run.retrospective && (
        <InterimPanel name={name} />
      )}

      <Section title='Flips' testId='experiment-flips'>
        <div className='flex flex-wrap gap-2'>
          <ToneBadge tone='neutral'>Sent {flips.sent}</ToneBadge>
          <ToneBadge tone={flips.confirmed > 0 ? "success" : "neutral"}>
            Confirmed {flips.confirmed}
          </ToneBadge>
          <ToneBadge tone={flips.unconfirmed > 0 ? "warning" : "neutral"}>
            Unconfirmed {flips.unconfirmed}
          </ToneBadge>
          <ToneBadge tone={flips.failed > 0 ? "danger" : "neutral"}>
            Failed {flips.failed}
          </ToneBadge>
          <ToneBadge tone='neutral'>Pending {flips.pending}</ToneBadge>
          <ToneBadge tone='neutral'>Cancelled {flips.cancelled}</ToneBadge>
        </div>
        {run.failed_flips.length > 0 && (
          <div className='-mx-4 mt-3 border-t' data-testid='experiment-failed-flips'>
            <CheckList
              checks={run.failed_flips.map((f, i) => ({
                id: `failed-flip-${i}`,
                title: `${f.unit} · ${f.state}`,
                status: "Failed",
                tone: "danger" as const,
                lines: [f.reason]
              }))}
            />
          </div>
        )}
      </Section>
      {run.design === "switchback" && run.periods && <SwitchbackSchedule periods={run.periods} />}
      {run.assign_by && (
        <GroupMembership assignBy={run.assign_by} membership={run.group_membership} />
      )}

      <GuardrailList guardrails={run.guardrails} design={run.design} assignBy={run.assign_by} />

      {run.problems.length > 0 && (
        <Section title='What the last check could not evaluate' testId='experiment-problems'>
          <div className='-mx-4'>
            <CheckList
              checks={run.problems.map((p, i) => ({
                id: `problem-${i}`,
                title: "Could not evaluate",
                status: "Problem",
                tone: "warning" as const,
                lines: [p]
              }))}
            />
          </div>
        </Section>
      )}
    </div>
  );
}
