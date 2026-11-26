import type React from "react";
import { cn } from "@/libs/shadcn/utils";
import { JOB_TYPE, JOB_TYPES, type JobType, TIME_RANGES, type TimeRange } from "./constants";

/** Generic segmented control. Scopes the surface it sits in. */
function Segmented<T extends string>({
  value,
  options,
  onChange,
  className
}: {
  value: T;
  options: { value: T; label: React.ReactNode }[];
  onChange: (v: T) => void;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "inline-flex items-center gap-0.5 border border-border bg-muted/40 p-0.5",
        className
      )}
    >
      {options.map((opt) => (
        <button
          key={opt.value}
          type='button'
          onClick={() => onChange(opt.value)}
          className={cn(
            "px-2.5 py-1 font-medium text-xs transition-colors",
            value === opt.value
              ? "bg-background text-foreground shadow-sm"
              : "text-muted-foreground hover:text-foreground"
          )}
        >
          {opt.label}
        </button>
      ))}
    </div>
  );
}

/** Time-range picker — Overview header + Runs filter bar. */
export const TimeRangePicker: React.FC<{
  value: TimeRange;
  onChange: (v: TimeRange) => void;
}> = ({ value, onChange }) => (
  <Segmented
    value={value}
    onChange={onChange}
    options={TIME_RANGES.map((r) => ({ value: r.value, label: r.label }))}
  />
);

/** "all" widens a job-type filter to every type. */
export type JobTypeChoice = JobType | "all";

/** Job-type filter — shared by Overview, Jobs, and Runs. */
export const JobTypeFilter: React.FC<{
  value: JobTypeChoice;
  onChange: (v: JobTypeChoice) => void;
}> = ({ value, onChange }) => (
  <Segmented<JobTypeChoice>
    value={value}
    onChange={onChange}
    options={[
      { value: "all", label: "All" },
      ...JOB_TYPES.map((t) => ({
        value: t,
        label: <span className='inline-flex items-center gap-1'>{JOB_TYPE[t].short}</span>
      }))
    ]}
  />
);

export { Segmented };
