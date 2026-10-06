import type React from "react";
import { cn } from "@/libs/shadcn/utils";

interface Props {
  /** The radio group name; unique per field so two fields never share one group. */
  group: string;
  testId: string;
  checked: boolean;
  onSelect: () => void;
  label: string;
  disabled?: boolean;
  /** Why the option is unavailable, shown on hover. */
  title?: string;
}

/** One choice in a short either/or row: a native radio, so arrow keys and screen readers just work. */
const OptionChip: React.FC<Props> = ({
  group,
  testId,
  checked,
  onSelect,
  label,
  disabled,
  title
}) => (
  <label
    data-testid={testId}
    data-state={checked ? "on" : "off"}
    title={title}
    className={cn(
      "inline-flex h-7 items-center rounded-md border px-2.5 font-medium text-xs transition-colors",
      "has-[:focus-visible]:border-ring has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-ring/50",
      checked
        ? "border-primary bg-primary/5 text-foreground"
        : "text-muted-foreground hover:bg-accent hover:text-accent-foreground",
      disabled ? "cursor-not-allowed opacity-50 hover:bg-transparent" : "cursor-pointer"
    )}
  >
    <input
      type='radio'
      name={group}
      checked={checked}
      onChange={onSelect}
      disabled={disabled}
      className='sr-only'
    />
    {label}
  </label>
);

export default OptionChip;
