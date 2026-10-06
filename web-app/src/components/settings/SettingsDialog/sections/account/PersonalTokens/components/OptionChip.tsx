import type React from "react";
import { cn } from "@/libs/shadcn/utils";

const VARIANTS = {
  outline: {
    base: "h-7 rounded-md border px-2.5 text-xs",
    on: "border-primary bg-primary/5 text-foreground",
    off: "text-muted-foreground hover:bg-accent hover:text-accent-foreground"
  },
  solid: {
    base: "h-8 rounded-md border px-3 text-[13px]",
    on: "border-primary bg-primary text-primary-foreground",
    off: "text-muted-foreground hover:bg-accent hover:text-accent-foreground"
  },
  segment: {
    base: "h-full min-w-0 justify-center rounded-md border border-transparent px-2 text-sm",
    on: "bg-background text-foreground shadow-sm",
    off: "text-muted-foreground hover:text-foreground"
  }
} as const;

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
  /**
   * `outline` is the small chip. `solid` is the token sheet's chip, filled when chosen.
   * `segment` is one part of a segmented control, whose track the caller draws.
   */
  variant?: keyof typeof VARIANTS;
}

/** One choice in a short either/or row: a native radio, so arrow keys and screen readers just work. */
const OptionChip: React.FC<Props> = ({
  group,
  testId,
  checked,
  onSelect,
  label,
  disabled,
  title,
  variant = "outline"
}) => (
  <label
    data-testid={testId}
    data-state={checked ? "on" : "off"}
    title={title}
    className={cn(
      "inline-flex items-center whitespace-nowrap font-medium transition-colors",
      "has-[:focus-visible]:border-ring has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-ring/50",
      VARIANTS[variant].base,
      checked ? VARIANTS[variant].on : VARIANTS[variant].off,
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
