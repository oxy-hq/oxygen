import { badgeLabel } from "@/components/Shell/TopBar/NotificationsBell/utils";
import { cn } from "@/libs/shadcn/utils";

/**
 * The red unread count, the way a dock icon carries one: on the bell, on each
 * app's home card and on its rail mark. Nothing at zero; "99+" past 99.
 * Position it from the caller (`className`) — it is usually pinned to a corner.
 * In dark mode the red is light, so the count is dark: white on it is 2.9:1.
 */
export function UnreadBadge({
  count,
  testId,
  className
}: {
  count: number;
  testId?: string;
  className?: string;
}) {
  const label = badgeLabel(count);
  if (!label) return null;
  return (
    <span
      data-testid={testId}
      className={cn(
        "pointer-events-none flex h-4 min-w-4 items-center justify-center rounded-full bg-destructive px-1 font-medium text-destructive-foreground text-xs tabular-nums leading-none dark:text-background",
        className
      )}
    >
      {label}
    </span>
  );
}
