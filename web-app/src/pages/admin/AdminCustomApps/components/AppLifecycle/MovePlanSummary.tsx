import { CircleX, TriangleAlert } from "lucide-react";
import type { ReactNode } from "react";
import { hardBlockers } from "@/hooks/api/customApps/useProductionMove";
import { errMessage } from "@/hooks/api/errMessage";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { MoveCadence, MovePlan } from "@/types/appPromote";
import { CheckNote } from "../AppConsole/BuildChecks/components/CheckNote";
import { PlanChecksLine } from "./PlanChecksLine";
import { checkBlockerText, planChecks } from "./planChecks";

/**
 * What a production move will do, said inside its confirm: the dry-run plan `oxyc
 * promote` prints. Blockers first — while one stands the verb stays disabled —
 * then the schedules it registers, changes or retires, the migrations applied before
 * the pointer moves, and the functions it adds, removes or changes.
 *
 * A plan that carries `checks` (the org's promote-checks gate is on, and the move is
 * not a rollback) also gets the required checks in one line, each with its status,
 * and the build's summary when a model wrote one, marked as a note. Its three check
 * blockers are drawn like the others, in the words of `productionGates`.
 */
export const MovePlanSummary = ({
  plan,
  isLoading,
  error
}: {
  plan: MovePlan | undefined;
  isLoading: boolean;
  error: unknown;
}) => {
  if (isLoading) {
    return (
      <p className='text-muted-foreground text-xs' data-testid='app-lifecycle-plan-loading'>
        Reading the plan…
      </p>
    );
  }
  if (error || !plan) {
    return (
      <Callout tone='danger' testId='app-lifecycle-plan-error'>
        Couldn't read the plan: {errMessage(error, "unknown error")}. Nothing moves until it can be
        read.
      </Callout>
    );
  }
  const blockers = hardBlockers(plan);
  const lines = planLines(plan);
  const checks = planChecks(plan);
  return (
    <div className='flex flex-col gap-2' data-testid='app-lifecycle-plan'>
      {blockers.map((b) => (
        <Callout key={b.code} tone='danger' testId={`app-lifecycle-plan-blocker-${b.code}`}>
          {checkBlockerText(b.code, checks) ?? b.message}
        </Callout>
      ))}
      {plan.warnings.map((w) => (
        <Callout key={w} tone='warn' testId='app-lifecycle-plan-warning'>
          {w}
        </Callout>
      ))}
      {checks && <PlanChecksLine checks={checks} />}
      {checks?.summary && (
        <CheckNote note={checks.summary} data-testid='app-lifecycle-plan-summary' />
      )}
      {lines.length === 0 ? (
        <p className='text-muted-foreground text-xs'>No function, schedule or migration changes.</p>
      ) : (
        <ul className='flex list-disc flex-col gap-1 pl-4 text-muted-foreground text-xs'>
          {lines.map(({ key, text }) => (
            <li key={key} data-testid={`app-lifecycle-plan-${key}`}>
              {text}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
};

const cadence = (c: MoveCadence) => `${c.cron} (${c.timezone})`;
const names = (list: string[]) => list.join(", ");

/** One line per kind of change the plan lists, in the order an operator weighs them. */
function planLines(plan: MovePlan): { key: string; text: ReactNode }[] {
  const lines: { key: string; text: ReactNode }[] = [];
  const s = plan.schedules;
  for (const r of s?.removed ?? []) {
    lines.push({
      key: `schedule-removed-${r.function}`,
      text: `Retires the schedule of ${r.function}, ${cadence(r)}.`
    });
  }
  for (const a of s?.added ?? []) {
    lines.push({
      key: `schedule-added-${a.function}`,
      text: `Registers ${a.function} on ${cadence(a)}.`
    });
  }
  for (const c of s?.changed ?? []) {
    lines.push({
      key: `schedule-changed-${c.function}`,
      text: `Moves ${c.function} from ${cadence(c.from)} to ${cadence(c.to)}.`
    });
  }
  const files = [...plan.migrations.oltp, ...plan.migrations.airhouse];
  if (files.length > 0 && plan.migrations.when === "before_move") {
    lines.push({
      key: "migrations",
      text: `Applies ${files.length} migration${files.length === 1 ? "" : "s"} before it goes live: ${names(files)}.`
    });
  }
  const f = plan.functions;
  if (f.added.length) lines.push({ key: "functions-added", text: `Adds ${names(f.added)}.` });
  if (f.removed.length)
    lines.push({ key: "functions-removed", text: `Removes ${names(f.removed)}.` });
  if (f.changed.length)
    lines.push({ key: "functions-changed", text: `Changes ${names(f.changed)}.` });
  return lines;
}

const Callout = ({
  tone,
  testId,
  children
}: {
  tone: "danger" | "warn";
  testId: string;
  children: ReactNode;
}) => {
  const Icon = tone === "danger" ? CircleX : TriangleAlert;
  const t = ADMIN_TONE[tone];
  return (
    <div
      className={cn("flex items-start gap-1.5 px-2.5 py-2 text-xs ring-1 ring-inset", t.bg, t.ring)}
      data-testid={testId}
    >
      <Icon className={cn("mt-px size-3 shrink-0", t.text)} />
      <span className='text-muted-foreground'>{children}</span>
    </div>
  );
};
