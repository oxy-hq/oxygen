import { Building2, Check, Lock } from "lucide-react";
import { cn } from "@/libs/shadcn/utils";
import type { AppVisibility } from "@/types/appAccess";

const OPTIONS: {
  value: AppVisibility;
  label: string;
  detail: string;
  icon: typeof Building2;
}[] = [
  {
    value: "org",
    label: "Everyone in the organization",
    detail: "Anyone who can sign in to this organization sees the app on their home page.",
    icon: Building2
  },
  {
    value: "members",
    label: "Only people you choose",
    detail: "The app is hidden from everyone else — they won't see it listed at all.",
    icon: Lock
  }
];

/**
 * The visibility switch, as two explicit cards rather than a toggle.
 *
 * A toggle would need a label that reads correctly in both states, and every
 * phrasing of that lost something ("Restricted" doesn't say restricted to whom).
 * Two cards let each option state its own consequence, which is the thing an admin
 * is actually deciding between.
 */
export function VisibilityChoice({
  value,
  onChange,
  reference = false
}: {
  value: AppVisibility;
  onChange: (v: AppVisibility) => void;
  /** The label as the reference design's capitalised section label (the home's dialog). */
  reference?: boolean;
}) {
  return (
    <fieldset className='flex flex-col gap-3'>
      <legend
        className={
          reference
            ? "mb-3 font-semibold text-ref-xs text-subtle-foreground uppercase tracking-eyebrow"
            : "mb-3 font-medium text-sm"
        }
      >
        Visibility
      </legend>
      <div className='grid gap-2 sm:grid-cols-2'>
        {OPTIONS.map((option) => {
          const selected = value === option.value;
          const Icon = option.icon;
          return (
            <label
              key={option.value}
              className={cn(
                "relative flex cursor-pointer flex-col gap-1.5 rounded-lg border p-3 transition-colors",
                "focus-within:ring-2 focus-within:ring-ring focus-within:ring-offset-1",
                selected
                  ? "border-primary bg-primary/5"
                  : "border-border hover:border-muted-foreground/40 hover:bg-muted/40"
              )}
            >
              <input
                type='radio'
                name='app-visibility'
                value={option.value}
                checked={selected}
                onChange={() => onChange(option.value)}
                className='sr-only'
                data-testid={`app-access-visibility-${option.value}`}
              />
              <div className='flex items-center gap-2'>
                <Icon
                  className={cn(
                    "size-4 shrink-0",
                    selected ? "text-primary" : "text-muted-foreground"
                  )}
                  aria-hidden
                />
                <span className='font-medium text-sm'>{option.label}</span>
                {selected && <Check className='ml-auto size-4 shrink-0 text-primary' aria-hidden />}
              </div>
              <p className='text-muted-foreground text-xs leading-relaxed'>{option.detail}</p>
            </label>
          );
        })}
      </div>
    </fieldset>
  );
}
