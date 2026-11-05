import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import { Select, SelectContent, SelectItem, SelectTrigger } from "@/components/ui/shadcn/select";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/shadcn/toggle-group";
import { bpsToPercent, type RulesDraft, splitTotalBps, TOTAL_BPS } from "./rules";

/** "Everyone else": one variant, or a percentage split across the variants. */
export function FallthroughEditor({
  variants,
  draft,
  onChange
}: {
  variants: string[];
  draft: RulesDraft;
  onChange: (next: RulesDraft) => void;
}) {
  const total = splitTotalBps(draft.percents);
  const totalOk = total === TOTAL_BPS;
  return (
    <div className='space-y-3'>
      <ToggleGroup
        type='single'
        variant='outline'
        size='sm'
        className='justify-start'
        value={draft.mode}
        onValueChange={(mode) => mode && onChange({ ...draft, mode: mode as RulesDraft["mode"] })}
      >
        <ToggleGroupItem
          value='variant'
          className='px-3 text-xs'
          data-testid='feature-flag-mode-variant'
        >
          One variant
        </ToggleGroupItem>
        <ToggleGroupItem
          value='split'
          className='px-3 text-xs'
          data-testid='feature-flag-mode-split'
        >
          Split
        </ToggleGroupItem>
      </ToggleGroup>

      {draft.mode === "variant" ? (
        <Select value={draft.variant} onValueChange={(variant) => onChange({ ...draft, variant })}>
          <SelectTrigger
            size='sm'
            className='w-40 text-xs'
            data-testid='feature-flag-fallthrough-variant'
          >
            <span className='truncate font-mono'>{draft.variant}</span>
          </SelectTrigger>
          <SelectContent>
            {variants.map((v) => (
              <SelectItem key={v} value={v} className='font-mono text-xs'>
                {v}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      ) : (
        <div className='space-y-2'>
          {variants.map((v) => (
            <div key={v} className='flex items-center gap-2 text-xs'>
              <Label
                htmlFor={`feature-flag-split-${v}`}
                className='w-28 truncate font-mono text-xs'
              >
                {v}
              </Label>
              <Input
                id={`feature-flag-split-${v}`}
                inputMode='decimal'
                value={draft.percents[v] ?? ""}
                onChange={(e) =>
                  onChange({ ...draft, percents: { ...draft.percents, [v]: e.target.value } })
                }
                className='h-8 w-20 text-right text-xs tabular-nums'
                data-testid={`feature-flag-split-${v}`}
              />
              <span className='text-muted-foreground'>%</span>
            </div>
          ))}
          <p
            className={totalOk ? "text-muted-foreground text-xs" : "text-destructive text-xs"}
            data-testid='feature-flag-split-total'
          >
            Total {total === null ? "—" : bpsToPercent(total)}%{totalOk ? "" : " — must be 100"}
          </p>
        </div>
      )}
    </div>
  );
}
