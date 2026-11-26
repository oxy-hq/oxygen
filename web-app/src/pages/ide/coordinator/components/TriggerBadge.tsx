import type React from "react";
import { cn } from "@/libs/shadcn/utils";
import { normalizeTrigger, TRIGGER, type Trigger } from "./constants";

/**
 * Trigger-source badge — `scheduled` / `manual` / `backfill`. The run log
 * uses this in place of the raw source-type so an operator can scan "what
 * fired this?" in one glance. Renders nothing for runs predating the tag.
 */
export const TriggerBadge: React.FC<{
  trigger: Trigger | string | null | undefined;
  /** `chip` = tinted pill with label; `icon` = bare colored icon. */
  variant?: "chip" | "icon";
  className?: string;
}> = ({ trigger, variant = "chip", className }) => {
  const key = normalizeTrigger(typeof trigger === "string" ? trigger : (trigger ?? undefined));
  if (!key) {
    return variant === "chip" ? (
      <span className={cn("text-muted-foreground text-xs", className)}>—</span>
    ) : null;
  }
  const meta = TRIGGER[key];
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
      {meta.label}
    </span>
  );
};
