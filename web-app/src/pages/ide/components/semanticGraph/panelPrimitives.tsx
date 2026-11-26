import { Info } from "lucide-react";
import type { ReactNode } from "react";
import { TooltipWrapper } from "@/components/ui/shadcn/utils/with-tooltip";
import { cn } from "@/libs/shadcn/utils";
import type { DriverForm } from "@/types/metricTree";

/**
 * Tooltip copy for a driver-edge functional form, shared by any surface that
 * shows a propagated impact's `form` — the World Model what-if panel and the
 * Metric Tree scenario node both walk the same edges, so they must not drift
 * into two descriptions of the same math.
 *
 * The name reads link-first, like the backend's: `linear-log` is a LINEAR target
 * (a fixed change) responding to the LOG of the driver (a percentage change).
 * `log-linear` is the mirror. Both sentences below used to be the wrong way
 * round.
 *
 * DIRECTION IS NOT PART OF THE FORM. Nothing in the fit constrains a
 * coefficient's sign: `quadratic` turns downward only when b₂ < 0, `inverse`
 * rises towards a ceiling only when b < 0, and `sqrt` gives diminishing returns
 * only when b > 0 — with the opposite sign each one is the mirror image, and
 * OLS will happily return it. So these sentences describe the SHAPE and leave
 * which way it points to the number and the profile shown beside them, both of
 * which state it from the fit rather than from the form's name.
 */
export const FORM_HELP: Record<DriverForm, string> = {
  linear: "Linear: a fixed change in the driver maps to a fixed change in the target.",
  "log-log":
    "Log-log: a percentage change in the driver maps to a percentage change in the target.",
  "log-linear":
    "Log-linear: a fixed change in the driver maps to a percentage change in the target.",
  "linear-log":
    "Linear-log: a percentage change in the driver maps to a fixed change in the target.",
  quadratic:
    "Quadratic: the driver's effect changes at a steady rate, so the curve turns once — past that point the effect reverses. The coefficient's sign says which way.",
  cubic:
    "Cubic: an S-curve, flat then steep then flat again. It can turn twice; the coefficients say where and in which direction.",
  sqrt: "Square root: the effect changes fastest near zero and flattens out, and unlike a log it still counts the days the driver sat at zero.",
  inverse:
    "Inverse: the target approaches a level it never reaches, moving less and less as the driver grows. The coefficient's sign says whether it approaches from above or below.",
  "linear-log-quadratic":
    "Linear-log-quadratic: a percentage-scaled response that can turn, with the turning point at a percentage of today's driver rather than at a fixed level."
};

export const CONFIDENCE_HELP = "The model's confidence in this edge's coefficient.";

/** Format a measure value for display: trim trailing float noise, cap decimals. */
export function formatMeasureValue(raw: string): string {
  const n = Number(raw);
  if (!Number.isFinite(n)) return raw;
  if (Number.isInteger(n)) return n.toLocaleString();
  // 7 significant digits to shed float noise, strip trailing zeros, then the
  // toLocaleString below caps display at 4 decimal places.
  const formatted = n.toPrecision(7).replace(/\.?0+$/, "");
  return Number(formatted).toLocaleString(undefined, { maximumFractionDigits: 4 });
}

export function SectionSpinner({ label = "loading…" }: { label?: string }) {
  return (
    <div className='flex items-center gap-2 py-2 font-mono text-[10px] text-muted-foreground'>
      <svg
        className='size-3 animate-spin'
        viewBox='0 0 24 24'
        fill='none'
        stroke='currentColor'
        strokeWidth={2.5}
      >
        <path d='M12 2v4M12 18v4M4.93 4.93l2.83 2.83M16.24 16.24l2.83 2.83M2 12h4M18 12h4M4.93 19.07l2.83-2.83M16.24 7.76l2.83-2.83' />
      </svg>
      {label}
    </div>
  );
}

export function SectionHeader({
  title,
  subtitle,
  color = "default"
}: {
  title: string;
  subtitle?: string;
  color?: "default" | "green" | "violet";
}) {
  return (
    <div className='flex items-baseline justify-between gap-2'>
      <span
        className={cn(
          "text-[9.5px] uppercase tracking-wider",
          color === "green" && "text-status-success",
          color === "violet" && "text-[color:var(--vis-purple)]",
          color === "default" && "text-foreground"
        )}
      >
        {title}
      </span>
      {subtitle && (
        <span className='shrink-0 truncate text-[9.5px] text-muted-foreground'>{subtitle}</span>
      )}
    </div>
  );
}

export function Row({
  children,
  onClick,
  className
}: {
  children: React.ReactNode;
  onClick?: () => void;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "flex min-w-0 items-center gap-2 border border-border-subtle bg-field px-2 py-1.5 font-mono text-xs",
        onClick && "cursor-pointer transition-colors hover:border-info/60",
        className
      )}
      onClick={onClick}
    >
      {children}
    </div>
  );
}

/**
 * Thin proportion bar for ranking dense rows at a glance — the eye should be
 * able to rank magnitudes without reading every number. `fraction` is clamped
 * to [0, 1]; the width is data-driven so it stays inline rather than an
 * arbitrary Tailwind size.
 */
export function MagnitudeBar({ fraction, className }: { fraction: number; className?: string }) {
  const pct = Math.max(0, Math.min(1, Number.isFinite(fraction) ? fraction : 0)) * 100;
  return (
    <div className={cn("h-0.5 w-full overflow-hidden rounded-full bg-muted", className)}>
      <div className='h-full rounded-full bg-info' style={{ width: `${pct}%` }} />
    </div>
  );
}

/** A hoverable ⓘ that moves always-on fine print into a tooltip. */
export function InfoTip({ content }: { content: string }) {
  return (
    <TooltipWrapper tooltip={content} className='max-w-64 text-[11px] leading-relaxed'>
      <span className='inline-flex cursor-help align-middle text-muted-foreground transition-colors hover:text-foreground'>
        <Info className='size-3' />
      </span>
    </TooltipWrapper>
  );
}

/** A terse metadata chip (e.g. `linear`, `high`), optionally explained on hover. */
export function MetaBadge({ children, tooltip }: { children: ReactNode; tooltip?: string }) {
  const badge = (
    <span className='border border-border-subtle px-1 py-px font-mono text-[9px] text-muted-foreground uppercase tracking-wide'>
      {children}
    </span>
  );
  return tooltip ? <TooltipWrapper tooltip={tooltip}>{badge}</TooltipWrapper> : badge;
}
