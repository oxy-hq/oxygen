import type { ReactNode } from "react";
import { cn } from "@/libs/shadcn/utils";

/**
 * One panel of the app console — the only content container this surface has.
 *
 * It replaces `DossierSection`, and the difference is the whole direction: a dossier
 * section is an **accordion**, collapsed by default, stacked in a side panel beside a
 * preview iframe that owned the stage. Nine of them meant an operator opened an app and
 * saw nine closed rows and a website. A panel is simply **open**. If something is not
 * worth showing, it does not get a panel; it does not get a collapsed one.
 *
 * The `question` prop is the part worth keeping honest. Each panel names the operator
 * question it answers, from the six this console was designed against — so a reviewer
 * can check the claim "one viewport answers all six" by reading the eyebrows, and a
 * panel that cannot name its question is a panel that has not earned its place.
 */
export const ConsolePanel = ({
  id,
  title,
  question,
  actions,
  children,
  className
}: {
  /** Stable id — drives the testid, so renaming the visible title cannot break a test. */
  id: string;
  title: string;
  /** The operator question this panel answers, e.g. "Q4 — who can open this app". */
  question?: string;
  /** One control at most. A panel needing a toolbar is two panels. */
  actions?: ReactNode;
  children: ReactNode;
  className?: string;
}) => (
  <section
    data-testid={`apps-console-panel-${id}`}
    className={cn("flex min-w-0 flex-col rounded-lg border border-border/60 bg-card", className)}
  >
    <header className='flex items-baseline justify-between gap-3 border-border/60 border-b px-3 py-2'>
      <div className='flex min-w-0 items-baseline gap-2'>
        <h3 className='font-semibold text-sm'>{title}</h3>
        {question ? (
          <span className='truncate font-medium text-[10px] text-muted-foreground uppercase tracking-[0.14em]'>
            {question}
          </span>
        ) : null}
      </div>
      {actions ? <div className='flex shrink-0 items-center gap-1.5'>{actions}</div> : null}
    </header>
    <div className='min-w-0 flex-1 p-3'>{children}</div>
  </section>
);

/**
 * A label/value row — the shape most of this console's content takes.
 *
 * Values are `tabular-nums` and right-aligned so a column of sizes, counts and ids lines
 * up at the digit down a panel, which is what makes a stack of these scannable rather
 * than merely present.
 */
export const PanelRow = ({
  label,
  children,
  mono,
  "data-testid": dataTestId
}: {
  label: string;
  children: ReactNode;
  /** For ids, slugs, shas and paths — anything an operator copies. */
  mono?: boolean;
  "data-testid"?: string;
}) => (
  <div data-testid={dataTestId} className='flex items-baseline justify-between gap-3 py-1 text-xs'>
    <span className='shrink-0 text-muted-foreground'>{label}</span>
    <span className={cn("min-w-0 truncate text-right", mono && "font-mono tabular-nums")}>
      {children}
    </span>
  </div>
);
