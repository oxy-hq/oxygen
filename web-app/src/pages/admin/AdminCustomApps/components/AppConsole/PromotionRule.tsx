import { Switch } from "@/components/ui/shadcn/switch";
import { useSetPromotionRule } from "@/hooks/api/customApps/usePromotionRule";
import type { CustomApp } from "@/types/apps";
import { secondPersonConsequence } from "./productionGates";

/**
 * The app's four-eyes rule (`require_second_person_to_promote`), in the panel that
 * says how CI ships the app — the rule is mostly about CI: with it on, a promoting
 * publish never moves production, because its publisher is its promoter.
 *
 * The sentence under the switch says what the current state *does*, not what the
 * switch is called, so turning it on or off is a decision read before it is made.
 * While the change is in flight the switch shows where it is going, and holds.
 */
export const PromotionRule = ({ app }: { app: CustomApp }) => {
  const set = useSetPromotionRule(app.id);
  const saved = app.require_second_person_to_promote ?? false;
  const on = set.isPending && set.variables !== undefined ? set.variables : saved;
  const id = `apps-console-ci-four-eyes-${app.id}`;

  return (
    <div
      className='space-y-1 border-border/60 border-t pt-2'
      data-testid='apps-console-ci-four-eyes'
    >
      <div className='flex items-center justify-between gap-3'>
        <label htmlFor={id} className='font-medium text-xs'>
          Require a second person to promote
        </label>
        <Switch
          id={id}
          checked={on}
          disabled={set.isPending}
          onCheckedChange={(next) => set.mutate(next)}
          data-testid='apps-console-ci-four-eyes-switch'
        />
      </div>
      <p
        className='text-muted-foreground text-xs'
        data-testid='apps-console-ci-four-eyes-consequence'
      >
        {secondPersonConsequence(on)}
      </p>
    </div>
  );
};
