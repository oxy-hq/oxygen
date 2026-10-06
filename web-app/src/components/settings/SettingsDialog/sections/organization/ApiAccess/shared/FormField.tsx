import type { ReactNode } from "react";
import { Label } from "@/components/ui/shadcn/label";
import { cn } from "@/libs/shadcn/utils";

interface FormFieldProps {
  /** The `id` of the control this labels. Omit for a group (radios, a checklist). */
  htmlFor?: string;
  label: string;
  /** "optional" and the like, set quietly beside the label. */
  aside?: string;
  hint?: ReactNode;
  error?: string | null;
  className?: string;
  children: ReactNode;
}

/**
 * A label, its control, and one line underneath: the error when there is one,
 * the hint otherwise. Never both, so the line doesn't jump as someone types.
 */
export function FormField({
  htmlFor,
  label,
  aside,
  hint,
  error,
  className,
  children
}: FormFieldProps) {
  return (
    <div className={cn("flex min-w-0 flex-col gap-1.5", className)}>
      <div className='flex items-baseline gap-2'>
        {htmlFor ? (
          <Label htmlFor={htmlFor} className='text-xs'>
            {label}
          </Label>
        ) : (
          // A group has no single control to point a <label> at; its own
          // fieldset or aria-label names it for assistive tech.
          <p className='font-medium text-xs leading-none'>{label}</p>
        )}
        {aside && <span className='text-muted-foreground text-xs'>{aside}</span>}
      </div>
      {children}
      {error ? (
        <p className='text-destructive text-xs' role='alert'>
          {error}
        </p>
      ) : (
        hint && <p className='text-muted-foreground text-xs leading-relaxed'>{hint}</p>
      )}
    </div>
  );
}

interface ChoiceRowProps {
  /** The radio group's name; unique per picker so two never share one group. */
  group: string;
  checked: boolean;
  onSelect: () => void;
  label: string;
  hint?: ReactNode;
  disabled?: boolean;
  testId: string;
}

/** One option of a small radio list. A native radio, so arrow keys just work. */
export function ChoiceRow({
  group,
  checked,
  onSelect,
  label,
  hint,
  disabled,
  testId
}: ChoiceRowProps) {
  return (
    <label
      data-testid={testId}
      className={cn(
        "flex cursor-pointer items-start gap-2 rounded-md px-2 py-1.5 text-xs hover:bg-accent",
        checked && "bg-accent",
        disabled && "cursor-not-allowed opacity-50 hover:bg-transparent"
      )}
    >
      <input
        type='radio'
        name={group}
        checked={checked}
        onChange={onSelect}
        disabled={disabled}
        className='mt-0.5 size-3.5 shrink-0 accent-primary'
      />
      <span className='flex min-w-0 flex-col gap-0.5'>
        <span className='font-medium'>{label}</span>
        {hint && <span className='text-muted-foreground leading-relaxed'>{hint}</span>}
      </span>
    </label>
  );
}
