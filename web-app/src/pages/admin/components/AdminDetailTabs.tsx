import type { ReactNode } from "react";
import {
  focusRingInset,
  segmentOff,
  segmentOn,
  segmentRow
} from "@/components/ui/reference/tokens";
import { cn } from "@/libs/shadcn/utils";

/**
 * Uppercase tab bar for an Admin detail surface. Restrained: a single
 * pixel underline marks the active tab; a dot may signal a
 * tab that has work waiting (e.g. "Members (2 pending invites)").
 *
 * Controlled component — state lives on the page. Keeps the URL the
 * sole source of truth for which entity is being inspected, while the
 * tab is local UI state.
 */
export const AdminDetailTabs = <T extends string>({
  value,
  onChange,
  tabs
}: {
  value: T;
  onChange: (next: T) => void;
  tabs: Array<{
    id: T;
    label: string;
    /** Optional count rendered after the label in tabular-nums. */
    count?: number;
    /** Render a small status dot when the tab carries unresolved work. */
    notify?: boolean;
  }>;
}) => (
  <div>
    <nav className={cn(segmentRow, "max-w-full flex-wrap")}>
      {tabs.map((tab) => {
        const active = tab.id === value;
        return (
          <button
            type='button'
            key={tab.id}
            onClick={() => onChange(tab.id)}
            aria-current={active ? "page" : undefined}
            className={cn(
              "group relative flex min-h-7 items-center gap-1.5 px-2.5 text-[11px] uppercase tracking-eyebrow transition-colors",
              focusRingInset,
              active ? segmentOn : segmentOff
            )}
          >
            <span>{tab.label}</span>
            {tab.count !== undefined ? (
              <span
                className={cn(
                  "rounded-full bg-status-neutral-bg px-1.5 py-0.5 font-medium text-[10px] tabular-nums tracking-normal",
                  active ? "text-foreground" : "text-muted-foreground"
                )}
              >
                {tab.count}
              </span>
            ) : null}
            {tab.notify ? <span className='size-1.5 rounded-full bg-primary' aria-hidden /> : null}
          </button>
        );
      })}
    </nav>
  </div>
);

/**
 * Wraps the body of the active tab with consistent vertical spacing so
 * every detail page renders the same rhythm.
 */
export const AdminDetailTabPanel = ({ children }: { children: ReactNode }) => (
  <div className='space-y-6 pt-6'>{children}</div>
);
