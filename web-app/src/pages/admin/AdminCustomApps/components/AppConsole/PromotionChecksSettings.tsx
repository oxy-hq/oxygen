import type { ReactNode } from "react";
import { Switch } from "@/components/ui/shadcn/switch";
import { useSetPromotionChecks } from "@/hooks/api/customApps/usePromotionChecks";
import type { CustomApp } from "@/types/apps";
import {
  gateAfter,
  PROMOTION_SWITCHES,
  type PromotionField,
  promotionGate,
  REQUIRABLE_CHECKS,
  requireCheck,
  setPromotionField
} from "./promotionChecksModel";

/**
 * Which checks hold this app's Promote, and what follows one: a row and a switch for
 * each check an app can require, then the three rules around them. Beside the
 * four-eyes rule, in the panel that says how the app ships.
 *
 * Nothing renders while the org's `promote_checks_gate` flag is off. An app on the
 * defaults (`required: null`) shows the defaults; its first change to a check sends
 * the list it then has. While a save is in flight the switches show where it is going,
 * and hold.
 */
export const PromotionChecksSettings = ({ app }: { app: CustomApp }) => {
  const set = useSetPromotionChecks(app.id);
  const saved = promotionGate(app.promotion_checks);
  if (!saved) return null;
  const gate = set.isPending && set.variables ? gateAfter(saved, set.variables) : saved;
  const id = (name: string) => `apps-console-ci-checks-${name}-${app.id}`;

  const row = (
    name: string,
    label: string,
    checked: boolean,
    send: (on: boolean) => void,
    idle = false
  ) => (
    <Row key={name} id={id(name)} name={name} label={label}>
      <Switch
        id={id(name)}
        checked={checked}
        disabled={set.isPending || idle}
        onCheckedChange={send}
        data-testid={`apps-console-ci-checks-${name}-switch`}
      />
    </Row>
  );
  const rule = (name: PromotionField, on: boolean, idle = false) =>
    row(name, LABEL[name], on, (next) => set.mutate(setPromotionField(gate, name, next)), idle);

  return (
    <div
      className='flex flex-col gap-1 border-border/60 border-t pt-2'
      data-testid='apps-console-ci-checks'
    >
      <Eyebrow>Promote waits for</Eyebrow>
      {REQUIRABLE_CHECKS.map((check) =>
        row(check.kind, check.label, gate.effective.includes(check.kind), (on) =>
          set.mutate(requireCheck(gate, check.kind, on))
        )
      )}
      {rule("seen_required", gate.seen_required)}
      <Eyebrow>After promote</Eyebrow>
      {rule("production_smoke", gate.production_smoke)}
      {/* Nothing to roll back on without the smoke: the switch rests, and reads off. */}
      {rule("auto_rollback", gate.auto_rollback && gate.production_smoke, !gate.production_smoke)}
    </div>
  );
};

const LABEL = Object.fromEntries(PROMOTION_SWITCHES.map((s) => [s.field, s.label])) as Record<
  PromotionField,
  string
>;

const Eyebrow = ({ children }: { children: ReactNode }) => (
  <span className='pt-1 text-[10px] text-muted-foreground uppercase tracking-[0.16em]'>
    {children}
  </span>
);

const Row = ({
  id,
  name,
  label,
  children
}: {
  id: string;
  name: string;
  label: string;
  children: ReactNode;
}) => (
  <div
    className='flex items-center justify-between gap-3'
    data-testid={`apps-console-ci-checks-${name}`}
  >
    <label htmlFor={id} className='font-medium text-xs'>
      {label}
    </label>
    {children}
  </div>
);
