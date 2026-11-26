import type React from "react";
import { cn } from "@/libs/shadcn/utils";
import { JOB_TYPE, type JobType } from "./constants";

/**
 * Job-type badge — icon + color for Agent / DAG / ELT. Job type is a filter,
 * not a tab, so this chip is what tells the three types apart everywhere.
 */
export const JobTypeBadge: React.FC<{
  type: JobType;
  /** "chip" = tinted pill with label; "icon" = bare colored icon. */
  variant?: "chip" | "icon";
  short?: boolean;
  className?: string;
}> = ({ type, variant = "chip", short, className }) => {
  const meta = JOB_TYPE[type];
  const Icon = meta.icon;

  if (variant === "icon") {
    return <Icon className={cn("h-4 w-4 shrink-0", meta.fg, className)} title={meta.label} />;
  }

  return (
    <span
      className={cn(
        "inline-flex items-center gap-1 px-1.5 py-0.5 font-medium text-xs",
        meta.tint,
        className
      )}
    >
      <Icon className='h-3 w-3 shrink-0' />
      {short ? meta.short : meta.label}
    </span>
  );
};
