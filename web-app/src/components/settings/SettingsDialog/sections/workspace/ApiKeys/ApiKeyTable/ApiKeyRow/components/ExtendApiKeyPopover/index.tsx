import type React from "react";
import { useId, useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { Calendar } from "@/components/ui/shadcn/calendar";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/shadcn/popover";
import { Spinner } from "@/components/ui/shadcn/spinner";
import type { TokenEndpoints } from "@/hooks/api/apiKeys/tokenEndpoints";
import { useExtendApiKey } from "@/hooks/api/apiKeys/useApiKeyMutations";
import { cn } from "@/libs/shadcn/utils";
import { ApiKeyService } from "@/services/api/apiKey";
import type { TokenSummary } from "@/types/apiToken";
import {
  buildExtendRequest,
  DEFAULT_EXTEND_CHOICE,
  EXTEND_PRESETS,
  type ExtendChoice,
  firstPickableDay,
  previewExpiry,
  submitLabel
} from "./extendChoice";

interface Props {
  /** The token being extended: its name and current expiry are all the popover reads. */
  token: TokenSummary;
  /** Where the extension is sent: workspace keys, personal tokens, or a service account. */
  endpoints: TokenEndpoints;
  /** Off where an org's policy caps token lifetimes: "No expiry" isn't on offer there. */
  allowNoExpiry?: boolean;
  /** The button that opens the popover. Rendered as the Radix trigger (`asChild`). */
  children: React.ReactElement;
}

/** "Expires Oct 3, 2026" / "Expired Sep 3, 2026", plus the one fact that matters. */
const CurrentExpiry: React.FC<{ token: TokenSummary }> = ({ token }) => {
  const expired = ApiKeyService.isExpired(token.expires_at);
  const day = token.expires_at ? ApiKeyService.formatDay(token.expires_at) : null;
  return (
    <p className='text-muted-foreground text-xs'>
      {expired && day ? `Expired ${day}. ` : day ? `Expires ${day}. ` : ""}
      {expired
        ? "Extending makes it work again with the same secret."
        : "The secret stays the same, so nothing needs redeploying."}
    </p>
  );
};

interface OptionProps {
  /** The radio group name; unique per popover so two rows never share one group. */
  group: string;
  testId: string;
  checked: boolean;
  onSelect: () => void;
  label: string;
  hint?: string;
}

/** One row of the option list: a native radio, so arrow keys and screen readers just work. */
const ExtendOption: React.FC<OptionProps> = ({ group, testId, checked, onSelect, label, hint }) => (
  <label
    data-testid={testId}
    className={cn(
      "flex cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 text-xs hover:bg-accent",
      checked && "bg-accent"
    )}
  >
    <input
      type='radio'
      name={group}
      checked={checked}
      onChange={onSelect}
      className='size-3.5 accent-primary'
    />
    <span className='font-medium'>{label}</span>
    {hint && <span className='ml-auto text-muted-foreground tabular-nums'>{hint}</span>}
  </label>
);

const ExtendApiKeyPopover: React.FC<Props> = ({
  token,
  endpoints,
  allowNoExpiry = true,
  children
}) => {
  const [open, setOpen] = useState(false);
  const [choice, setChoice] = useState<ExtendChoice>(DEFAULT_EXTEND_CHOICE);
  const group = useId();
  const extend = useExtendApiKey(endpoints);

  const preview = previewExpiry(token, choice);
  const request = buildExtendRequest(choice);

  const handleOpenChange = (next: boolean) => {
    setOpen(next);
    if (next) setChoice(DEFAULT_EXTEND_CHOICE);
  };

  const submit = () => {
    if (!request) return;
    extend.mutate({ token, request }, { onSuccess: () => setOpen(false) });
  };

  return (
    <Popover open={open} onOpenChange={handleOpenChange}>
      <PopoverTrigger asChild>{children}</PopoverTrigger>
      <PopoverContent align='end' className='w-72 p-3' data-testid='api-key-extend-popover'>
        <div className='flex flex-col gap-3'>
          <div className='flex flex-col gap-1'>
            <h4 className='truncate font-medium text-sm'>Extend {token.name}</h4>
            <CurrentExpiry token={token} />
          </div>

          <fieldset className='flex flex-col gap-0.5 border-0 p-0'>
            <legend className='sr-only'>New expiry</legend>
            {EXTEND_PRESETS.map(({ days, label }) => (
              <ExtendOption
                key={days}
                group={group}
                testId={`api-key-extend-option-${days}`}
                checked={choice.kind === "days" && choice.days === days}
                onSelect={() => setChoice({ kind: "days", days })}
                label={label}
                hint={`until ${ApiKeyService.formatDay(ApiKeyService.extendedExpiry(token.expires_at, days))}`}
              />
            ))}
            <ExtendOption
              group={group}
              testId='api-key-extend-option-date'
              checked={choice.kind === "date"}
              onSelect={() => setChoice({ kind: "date" })}
              label='Pick a date'
              hint={
                choice.kind === "date" && choice.date
                  ? ApiKeyService.formatDay(choice.date)
                  : undefined
              }
            />
            {choice.kind === "date" && (
              <Calendar
                mode='single'
                selected={choice.date}
                onSelect={(date) => setChoice({ kind: "date", date })}
                disabled={{ before: firstPickableDay() }}
                defaultMonth={choice.date ?? firstPickableDay()}
                className='self-center p-1'
                data-testid='api-key-extend-calendar'
              />
            )}
            {allowNoExpiry && (
              <ExtendOption
                group={group}
                testId='api-key-extend-option-never'
                checked={choice.kind === "never"}
                onSelect={() => setChoice({ kind: "never" })}
                label='No expiry'
                hint='never expires'
              />
            )}
          </fieldset>

          <div className='flex justify-end gap-2'>
            <Button
              variant='ghost'
              size='sm'
              className='h-7 px-2 text-xs'
              onClick={() => setOpen(false)}
              data-testid='api-key-extend-cancel'
            >
              Cancel
            </Button>
            <Button
              size='sm'
              className='h-7 px-2 text-xs'
              onClick={submit}
              disabled={!request || extend.isPending}
              data-testid='api-key-extend-submit'
            >
              {extend.isPending && <Spinner className='size-3' />}
              {submitLabel(preview)}
            </Button>
          </div>
        </div>
      </PopoverContent>
    </Popover>
  );
};

export default ExtendApiKeyPopover;
