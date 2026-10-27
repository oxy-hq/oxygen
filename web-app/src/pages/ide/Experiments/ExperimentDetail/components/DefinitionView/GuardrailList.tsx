import HowCalculated from "../ResultPane/components/HowCalculated";
import { filterText, guardrailText, measureLabel, percent } from "./describe";
import { orDash, type SafeDefinition } from "./safe";
import { Block, Chip, LabelledChip } from "./shared";

export default function GuardrailList({ def }: { def: SafeDefinition }) {
  const sub = def.substitution_check;
  const guardrails = def.guardrails ?? [];
  const d = def.design ?? {};
  const muted = "text-muted-foreground text-sm";
  return (
    <>
      <Block id='quality' title='Quality floors'>
        <div className='flex flex-wrap items-center gap-1.5'>
          <LabelledChip label='Data coverage at least'>
            {orDash(d.coverage_floor, percent)}
          </LabelledChip>
          <LabelledChip label='Switches landed at least'>
            {orDash(d.compliance_floor, percent)}
          </LabelledChip>
        </div>
        <HowCalculated title='What the floors do'>
          <p>
            At least {orDash(d.coverage_floor, percent)} of each window's days must have data, and
            at least {orDash(d.compliance_floor, percent)} of switches must land; otherwise the
            result is refused or flagged as measuring the attempt.
          </p>
        </HowCalculated>
      </Block>
      <Block id='substitution_check' title='Substitution check'>
        {sub?.sibling ? (
          <p className='text-sm'>
            Watches {measureLabel(sub.sibling)} <Chip>{sub.sibling}</Chip>
            {sub.filter?.length ? `, ${filterText(sub.filter)},` : ""} for cannibalisation of the
            outcome.
          </p>
        ) : (
          <p className={muted}>No substitution check.</p>
        )}
      </Block>
      <Block id='guardrails' title='Guardrails'>
        {guardrails.length === 0 ? (
          <p className={muted}>No guardrails registered.</p>
        ) : (
          <ul className='space-y-1 text-sm'>
            {guardrails.map((g) => (
              <li key={JSON.stringify(g.criteria)}>
                {guardrailText(g)} {g.criteria.type === "trigger" && <Chip>{g.criteria.when}</Chip>}
                {g.criteria.type === "anomaly" && <Chip>{g.criteria.measure}</Chip>}
              </li>
            ))}
          </ul>
        )}
      </Block>
    </>
  );
}
