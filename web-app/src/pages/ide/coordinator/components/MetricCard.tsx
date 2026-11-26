import type React from "react";
import { cn } from "@/libs/shadcn/utils";

type Tone = "default" | "success" | "warning" | "destructive" | "primary";

const toneText: Record<Tone, string> = {
  default: "text-foreground",
  success: "text-success",
  warning: "text-warning",
  destructive: "text-destructive",
  primary: "text-primary"
};

/**
 * A single health-metric card for the Overview header strip. "Healthy" is
 * not the same as "succeeded" — a tone lets a card go amber/red even when
 * nothing hard-failed (e.g. a cost spike).
 */
export const MetricCard: React.FC<{
  label: string;
  value: React.ReactNode;
  icon?: React.ElementType;
  tone?: Tone;
  hint?: string;
  /** Pulse the value — used for "running now". */
  live?: boolean;
}> = ({ label, value, icon: Icon, tone = "default", hint, live }) => (
  <div className='flex flex-col gap-1 border border-border bg-card px-4 py-3'>
    <div className='flex items-center gap-1.5 text-muted-foreground'>
      {Icon && <Icon className='h-3.5 w-3.5' />}
      <span className='font-medium text-xs uppercase tracking-wide'>{label}</span>
    </div>
    <div className='flex items-baseline gap-2'>
      <span
        className={cn(
          "font-semibold text-2xl tabular-nums leading-none",
          toneText[tone],
          live && "animate-pulse"
        )}
      >
        {value}
      </span>
      {hint && <span className='text-muted-foreground text-xs'>{hint}</span>}
    </div>
  </div>
);
