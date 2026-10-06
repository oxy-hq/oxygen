import { CalendarDays } from "lucide-react";
import type React from "react";
import { useId, useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { Calendar } from "@/components/ui/shadcn/calendar";
import { Label } from "@/components/ui/shadcn/label";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/shadcn/popover";
import { cn } from "@/libs/shadcn/utils";
import { ApiKeyService } from "@/services/api/apiKey";
import type { LifetimeCap } from "../accessDraft";
import {
  type ExpiryChoice,
  expiryOptions,
  expiryPreview,
  expiryProblem,
  pickableRange,
  presetAllowed
} from "../expiry";
import OptionChip from "./OptionChip";
import SheetRow from "./SheetRow";

interface Props {
  choice: ExpiryChoice;
  onChange: (choice: ExpiryChoice) => void;
  /** The tightest max lifetime among the orgs the token is narrowed to, if any. */
  cap: LifetimeCap | null;
  /** Prefix for testids, so each dialog that uses the field stays distinguishable. */
  testId?: string;
  /** For a dialog whose labels are set smaller than the default. */
  labelClassName?: string;
  /**
   * `sheet` sets the label in the gutter of a token sheet, beside filled chips. `stacked` is the
   * label above its chips, for a dialog laid out top to bottom.
   */
  variant?: "stacked" | "sheet";
}

const capNote = (cap: LifetimeCap): string =>
  `${cap.orgName} limits tokens to ${cap.days} day${cap.days === 1 ? "" : "s"}.`;

/** The date option's own control: a button that opens a calendar bounded by the cap. */
const DateChoice: React.FC<{
  date?: Date;
  cap: LifetimeCap | null;
  testId: string;
  onPick: (date?: Date) => void;
}> = ({ date, cap, testId, onPick }) => {
  const [open, setOpen] = useState(false);
  const range = pickableRange(cap);
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          type='button'
          variant='outline'
          size='sm'
          className='h-7 w-fit gap-1.5 px-2.5 text-xs'
          data-testid={`${testId}-date-button`}
        >
          <CalendarDays className='size-3.5' />
          {date ? ApiKeyService.formatDay(date) : "Choose a date"}
        </Button>
      </PopoverTrigger>
      <PopoverContent align='start' className='w-auto p-0'>
        <Calendar
          mode='single'
          selected={date}
          onSelect={(picked) => {
            onPick(picked);
            if (picked) setOpen(false);
          }}
          disabled={[{ before: range.before }, ...(range.after ? [{ after: range.after }] : [])]}
          defaultMonth={date ?? range.before}
          data-testid={`${testId}-calendar`}
        />
      </PopoverContent>
    </Popover>
  );
};

/** What the choice comes to, in one line: the date, "never", or why it can't be used. */
const Outcome: React.FC<{ choice: ExpiryChoice; cap: LifetimeCap | null; testId: string }> = ({
  choice,
  cap,
  testId
}) => {
  const problem = expiryProblem(choice, cap);
  if (problem) {
    return (
      <p className='text-destructive text-xs' data-testid={`${testId}-problem`}>
        {problem}
      </p>
    );
  }
  const expires = expiryPreview(choice);
  return (
    <p className='text-muted-foreground text-xs' data-testid={`${testId}-outcome`}>
      {expires === undefined
        ? "Choose the day the token stops working."
        : expires === null
          ? "Works until you revoke it."
          : `Stops working ${ApiKeyService.formatDay(expires)}. You can extend it later.`}
      {cap && ` ${capNote(cap)}`}
    </p>
  );
};

/**
 * 7, 30, 90 or 365 days, a date, or no expiry: how long a new token lives, for a personal token
 * and for a service account's alike. An org's max lifetime disables what exceeds it and is
 * offered as an option itself.
 */
const ExpiryField: React.FC<Props> = ({
  choice,
  onChange,
  cap,
  testId = "account-token-expiry",
  labelClassName,
  variant = "stacked"
}) => {
  const group = useId();
  const sheet = variant === "sheet";
  const chip = sheet ? "solid" : "outline";
  const choices = (
    <>
      <div className='flex flex-wrap items-center gap-1.5'>
        {expiryOptions(cap).map(({ days, label }) => (
          <OptionChip
            key={days}
            group={group}
            testId={`${testId}-${days}`}
            checked={choice.kind === "days" && choice.days === days}
            onSelect={() => onChange({ kind: "days", days })}
            label={label}
            variant={chip}
            disabled={!presetAllowed(days, cap)}
            title={cap && !presetAllowed(days, cap) ? capNote(cap) : undefined}
          />
        ))}
        <OptionChip
          group={group}
          testId={`${testId}-date`}
          checked={choice.kind === "date"}
          onSelect={() => onChange({ kind: "date" })}
          label='Custom date'
          variant={chip}
        />
        <OptionChip
          group={group}
          testId={`${testId}-never`}
          checked={choice.kind === "never"}
          onSelect={() => onChange({ kind: "never" })}
          label='No expiry'
          variant={chip}
          disabled={!!cap}
          title={cap ? capNote(cap) : undefined}
        />
      </div>
      {choice.kind === "date" && (
        <DateChoice
          date={choice.date}
          cap={cap}
          testId={testId}
          onPick={(date) => onChange({ kind: "date", date })}
        />
      )}
      <Outcome choice={choice} cap={cap} testId={testId} />
    </>
  );

  if (sheet) {
    return (
      <SheetRow label='Expires' labelId={group}>
        {/* `pt-0.5` centres the 32px chips on the 36px line the label sits on. */}
        <div role='radiogroup' aria-labelledby={group} className='flex flex-col gap-2 pt-0.5'>
          {choices}
        </div>
      </SheetRow>
    );
  }
  return (
    <fieldset className='flex min-w-0 flex-col gap-2 border-0 p-0'>
      <Label asChild className={cn(labelClassName)}>
        <legend>Expires</legend>
      </Label>
      {choices}
    </fieldset>
  );
};

export default ExpiryField;
