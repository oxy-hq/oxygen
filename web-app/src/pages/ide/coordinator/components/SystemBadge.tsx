import { Cog } from "lucide-react";
import type React from "react";
import { cn } from "@/libs/shadcn/utils";

/**
 * Indicator that a run / job is system-managed (e.g. the preagg refresh
 * worker's heartbeat cycles) rather than user-scheduled work. Renders in
 * place of the JobTypeBadge on rows where the source_type is system —
 * agent/dag/elt would misclassify these as user-owned.
 */
export const SystemBadge: React.FC<{
  variant?: "chip" | "icon";
  className?: string;
}> = ({ variant = "chip", className }) => {
  if (variant === "icon") {
    return (
      <span role='img' aria-label='System-managed daemon' title='System-managed daemon'>
        <Cog className={cn("h-4 w-4 shrink-0 text-muted-foreground", className)} />
      </span>
    );
  }
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1 bg-muted px-1.5 py-0.5",
        "font-medium text-muted-foreground text-xs",
        className
      )}
      title='System-managed daemon — not a user-scheduled job'
    >
      <Cog className='h-3 w-3 shrink-0' />
      System
    </span>
  );
};
