import type { ReactNode } from "react";
import { Card } from "@/components/ui/shadcn/card";
import { cn } from "@/libs/shadcn/utils";

/**
 * A titled card on the home: a header row with the title (and an optional item
 * on its right), a rule, then the body filling the rest of the height. The world
 * model card and the recent questions both use it, so the two cards in the lower
 * row have the same header height, rule and padding. A border and no shadow: the
 * card sits in the page, it does not float over it.
 */
export function HomePanel({
  title,
  titleId,
  right,
  children,
  className,
  testId
}: {
  title: ReactNode;
  titleId?: string;
  right?: ReactNode;
  children: ReactNode;
  className?: string;
  testId?: string;
}) {
  return (
    <Card
      className={cn("relative flex min-w-0 flex-col overflow-hidden shadow-none", className)}
      data-testid={testId}
    >
      <div className='flex h-12 shrink-0 items-center justify-between gap-3 border-b px-4'>
        <h2 id={titleId} className='min-w-0 truncate font-medium text-sm'>
          {title}
        </h2>
        {right}
      </div>
      <div className='flex min-h-0 flex-1 flex-col'>{children}</div>
    </Card>
  );
}
