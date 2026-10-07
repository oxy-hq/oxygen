import { useId } from "react";
import { Checkbox } from "@/components/ui/shadcn/checkbox";
import { Label } from "@/components/ui/shadcn/label";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/shadcn/toggle-group";
import type { FilterCounts, StandingFilter, TokenFilter } from "../tokenFilter";

const STANDING_CHOICES: { value: StandingFilter; label: string }[] = [
  { value: "all", label: "All" },
  { value: "staff", label: "Staff standing" },
  { value: "partner", label: "Partner standing" }
];

interface Props {
  filter: TokenFilter;
  /** How many rows each choice shows, and how many ended tokens hiding takes away. */
  counts: FilterCounts;
  onChange: (filter: TokenFilter) => void;
}

/**
 * Which tokens the table shows: all of them, or the ones carrying one standing, with or without
 * the ones that have ended. Each choice carries its count, so one that would show everything or
 * nothing says so before it is picked.
 */
export function StandingTokenFilters({ filter, counts, onChange }: Props) {
  const hideEndedId = useId();

  return (
    <div
      className='flex flex-wrap items-center justify-between gap-2'
      data-testid='admin-standing-tokens-filters'
    >
      <ToggleGroup
        type='single'
        size='sm'
        value={filter.standing}
        // Radix answers "" when the chosen item is pressed again: a filter is never unset.
        onValueChange={(value) =>
          value && onChange({ ...filter, standing: value as StandingFilter })
        }
        aria-label='Show tokens by standing'
        className='gap-0 rounded-md border border-border/60 bg-card p-0.5'
        data-testid='admin-standing-tokens-standing-filter'
      >
        {STANDING_CHOICES.map((choice) => (
          <ToggleGroupItem
            key={choice.value}
            value={choice.value}
            className='h-7 gap-1.5 px-2.5 text-xs data-[state=on]:bg-muted'
            data-testid={`admin-standing-tokens-filter-${choice.value}`}
          >
            {choice.label}
            <span className='text-muted-foreground tabular-nums'>
              {counts.standing[choice.value]}
            </span>
          </ToggleGroupItem>
        ))}
      </ToggleGroup>

      <div className='flex items-center gap-2'>
        <Checkbox
          id={hideEndedId}
          checked={filter.hideEnded}
          onCheckedChange={(checked) => onChange({ ...filter, hideEnded: checked === true })}
          data-testid='admin-standing-tokens-hide-ended'
        />
        <Label
          htmlFor={hideEndedId}
          className='cursor-pointer font-normal text-muted-foreground text-xs hover:text-foreground'
        >
          Hide expired and revoked ({counts.ended})
        </Label>
      </div>
    </div>
  );
}
