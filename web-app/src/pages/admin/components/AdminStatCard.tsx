import { ArrowDownRight, ArrowUpRight } from "lucide-react";
import type { ComponentType, ReactNode } from "react";
import { Link } from "react-router-dom";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "./adminTone";

/**
 * Operator-grade stat tile. One number, one label, one optional delta.
 * Wide enough to lay out three across the overview hub. Hover gives a
 * subtle one-pixel lift so the operator knows it's interactive when wired
 * to a destination route. Numbers always render in tabular-nums so a
 * column of cards stays aligned at the digit.
 *
 * Intentionally not a shadcn primitive — the calculus here is design
 * voice (typography hierarchy, the link affordance, the delta arrow)
 * rather than reusing a generic card.
 */
export const AdminStatCard = ({
  label,
  value,
  delta,
  trailing,
  icon: Icon,
  to,
  className
}: {
  label: string;
  value: ReactNode;
  /** Positive = up arrow + emerald; negative = down arrow + amber. */
  delta?: { value: number; suffix?: string };
  /** Optional trailing slot (e.g. a small badge). Rendered top-right. */
  trailing?: ReactNode;
  icon?: ComponentType<{ className?: string }>;
  /** When set, the card becomes a Link to this URL. */
  to?: string;
  className?: string;
}) => {
  const body = (
    <div
      className={cn(
        "group flex flex-col gap-4 rounded-lg border border-border/60 bg-card p-6 shadow-[0_1px_0_0_color-mix(in_srgb,var(--border)_60%,transparent)] transition-colors",
        to && "hover:border-foreground/20 hover:bg-card/80",
        className
      )}
    >
      <div className='flex items-start justify-between'>
        <div className='flex items-center gap-2 font-medium text-[10px] text-muted-foreground uppercase tracking-[0.14em]'>
          {Icon ? <Icon className='size-3.5' /> : null}
          {label}
        </div>
        {trailing}
      </div>
      <div className='flex items-baseline gap-3'>
        <span className='font-semibold text-4xl tabular-nums tracking-tight'>{value}</span>
        {delta !== undefined ? <Delta {...delta} /> : null}
      </div>
    </div>
  );

  if (to) {
    return (
      <Link to={to} className='block'>
        {body}
      </Link>
    );
  }
  return body;
};

const Delta = ({ value, suffix }: { value: number; suffix?: string }) => {
  if (value === 0) {
    return (
      <span className='inline-flex items-center gap-0.5 font-medium text-muted-foreground text-xs tabular-nums'>
        ±0{suffix ? ` ${suffix}` : ""}
      </span>
    );
  }
  const positive = value > 0;
  const Icon = positive ? ArrowUpRight : ArrowDownRight;
  return (
    <span
      className={cn(
        "inline-flex items-center gap-0.5 font-medium text-xs tabular-nums",
        positive ? ADMIN_TONE.ok.text : ADMIN_TONE.warn.text
      )}
    >
      <Icon className='size-3' />
      {positive ? "+" : ""}
      {value}
      {suffix ? ` ${suffix}` : ""}
    </span>
  );
};
