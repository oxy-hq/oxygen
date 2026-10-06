import type React from "react";
import { useId } from "react";
import { Input } from "@/components/ui/shadcn/input";
import { lifetimeLabel, lifetimePresets, sandboxExpiry } from "@/libs/sandboxAgentToken";
import { ApiKeyService } from "@/services/api/apiKey";
import type { SandboxAgentLimits } from "@/types/apiToken";
import { type LifetimeChoice, lifetimeHours, lifetimeProblem } from "../sandboxDraft";
import OptionChip from "./OptionChip";
import SheetRow from "./SheetRow";

interface Props {
  choice: LifetimeChoice;
  onChange: (choice: LifetimeChoice) => void;
  limits: SandboxAgentLimits;
}

const TEST_ID = "account-token-lifetime";

/** What the choice comes to, in one line: the moment it stops working, or why it can't be used. */
const Outcome: React.FC<Pick<Props, "choice" | "limits">> = ({ choice, limits }) => {
  const problem = lifetimeProblem(choice, limits);
  if (problem) {
    return (
      <p className='text-destructive text-xs' data-testid={`${TEST_ID}-problem`}>
        {problem}
      </p>
    );
  }
  const hours = lifetimeHours(choice);
  return (
    <p className='text-muted-foreground text-xs' data-testid={`${TEST_ID}-outcome`}>
      {hours === null
        ? `Enter how many hours the token should last, up to ${limits.max_hours}.`
        : `Stops working ${ApiKeyService.formatDate(sandboxExpiry(hours).toISOString())}. It can't be extended: create a new token when this one lapses.`}
    </p>
  );
};

/**
 * How long a sandbox agent token lives. Hours, where a personal token counts days: the token is
 * for one task, and its lifetime is fixed when it is minted. Takes the place of the expiry field
 * while the sandbox agent type is chosen.
 */
const SandboxLifetimeField: React.FC<Props> = ({ choice, onChange, limits }) => {
  const group = useId();
  return (
    <SheetRow label='Expires' labelId={group}>
      {/* `pt-0.5` centres the 32px chips on the 36px line the label sits on. */}
      <div role='radiogroup' aria-labelledby={group} className='flex flex-col gap-2 pt-0.5'>
        <div className='flex flex-wrap items-center gap-1.5'>
          {lifetimePresets(limits).map((hours) => (
            <OptionChip
              key={hours}
              group={group}
              testId={`${TEST_ID}-${hours}`}
              checked={choice.kind === "preset" && choice.hours === hours}
              onSelect={() => onChange({ kind: "preset", hours })}
              label={lifetimeLabel(hours)}
              variant='solid'
            />
          ))}
          <OptionChip
            group={group}
            testId={`${TEST_ID}-custom`}
            checked={choice.kind === "custom"}
            // Starts from the span already picked, so the box is never opened empty.
            onSelect={() => onChange({ kind: "custom", text: String(lifetimeHours(choice) ?? "") })}
            label='Custom'
            variant='solid'
          />
        </div>
        {choice.kind === "custom" && (
          <div className='flex items-center gap-2'>
            <Input
              value={choice.text}
              onChange={(event) => onChange({ kind: "custom", text: event.target.value })}
              inputMode='numeric'
              autoComplete='off'
              className='h-8 w-20'
              aria-label='Lifetime in hours'
              aria-invalid={lifetimeProblem(choice, limits) !== null}
              data-testid={`${TEST_ID}-custom-input`}
            />
            <span className='text-muted-foreground text-sm'>hours</span>
          </div>
        )}
        <Outcome choice={choice} limits={limits} />
      </div>
    </SheetRow>
  );
};

export default SandboxLifetimeField;
