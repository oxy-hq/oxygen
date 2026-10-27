import { type CheckItem, CheckList } from "@/pages/ide/Experiments/components/glance/CheckList";
import type { Tone } from "@/pages/ide/Experiments/components/glance/tone";
import type { DesignKind, GuardrailState, GuardrailStatus } from "@/types/experiment";
import HowCalculated from "../../ResultPane/components/HowCalculated";
import Section from "../../Section";
import { summariseUnits } from "../operations";

export const GUARDRAIL_LABEL: Record<GuardrailState, string> = {
  ok: "OK",
  breached: "Breached",
  not_checked: "Not checked yet",
  refused: "Could not be evaluated"
};
export const GUARDRAIL_TONE: Record<GuardrailState, Tone> = {
  ok: "success",
  breached: "danger",
  not_checked: "neutral",
  refused: "warning"
};

/** What "a unit" is for this design, in the platform's own words. */
function scopeOf(design: DesignKind, assignBy: string | null): string {
  if (design === "switchback")
    return "Each guardrail is checked at every unit during the on periods.";
  if (assignBy) return "Each guardrail is checked per group.";
  return "Each guardrail is checked per treated unit.";
}

function toCheck(g: GuardrailStatus, i: number): CheckItem {
  const s = summariseUnits(g.units);
  const lines: CheckItem["lines"] = [];
  if (g.detail) lines.push(g.detail);
  for (const u of s.breached) {
    lines.push(
      <span key={u.unit} data-testid='experiment-guardrail-breached-unit'>
        <strong>{u.unit}</strong> breached{u.detail ? `: ${u.detail}` : ""}
      </span>
    );
  }
  for (const u of s.unevaluated) {
    lines.push(
      `${u.unit}: ${GUARDRAIL_LABEL[u.status].toLowerCase()}${u.detail ? ` — ${u.detail}` : ""}`
    );
  }
  if (g.units.length > 0) {
    lines.push(
      <span key='ok' data-testid='experiment-guardrail-units-ok'>
        {s.ok} of {g.units.length} units OK
      </span>
    );
  }
  return {
    id: `guardrail-${i}`,
    title: g.criteria,
    status: GUARDRAIL_LABEL[g.status],
    tone: GUARDRAIL_TONE[g.status],
    lines
  };
}

/**
 * Guardrails are watched per unit, so an aggregate cannot hide one unit's
 * breach; one unit breaching trips the abort and the page names it. Units that
 * are fine are counted, not listed.
 */
export default function GuardrailList({
  guardrails,
  design,
  assignBy
}: {
  guardrails: GuardrailStatus[];
  design: DesignKind;
  assignBy: string | null;
}) {
  return (
    <Section title='Guardrails' testId='experiment-guardrails'>
      {guardrails.length === 0 ? (
        <p className='text-sm'>None registered.</p>
      ) : (
        <div className='flex flex-col gap-2'>
          <div className='-mx-4'>
            <CheckList checks={guardrails.map(toCheck)} />
          </div>
          <HowCalculated title='How this works'>
            <p data-testid='experiment-guardrail-scope'>
              {scopeOf(design, assignBy)} One breach trips the abort, which names the unit.
            </p>
          </HowCalculated>
        </div>
      )}
    </Section>
  );
}
