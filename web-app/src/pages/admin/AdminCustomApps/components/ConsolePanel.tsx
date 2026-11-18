import type { ReactNode } from "react";
import { cn } from "@/libs/shadcn/utils";
import { AdminInfo } from "@/pages/admin/components/AdminInfo";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";

/**
 * One panel of the app console — the only content container this surface has.
 *
 * It replaces `DossierSection`, an accordion collapsed by default. A panel is simply
 * **open**: if something is not worth showing it does not get a panel, not a closed one.
 *
 * Panels used to carry an eyebrow naming the operator question each answered ("Q1 — is
 * it broken"). That was a design-review device, and on screen it was one more line of
 * small caps per panel, twenty times down a page. The console is now split into pages
 * by the job an operator came for (`consolePages.ts`), which is what the questions were
 * standing in for; a panel says what it is with its title, and how it is measured with
 * `info`, behind an ⓘ.
 */
export const ConsolePanel = ({
  id,
  title,
  info,
  scope,
  actions,
  children,
  className
}: {
  /** Stable id — drives the testid, so renaming the visible title cannot break a test. */
  id: string;
  title: string;
  /** What the panel shows and how it is measured, behind an ⓘ beside the title. */
  info?: ReactNode;
  /**
   * Which environment the panel shows, when it is one environment's: "Production"
   * for panels that only read production, or the console's environment for panels
   * that follow its switch. `warn` when that is not production. Absent on panels
   * about the app as a whole (identity, domains, publishers).
   */
  scope?: { label: string; warn?: boolean };
  /** One control at most. A panel needing a toolbar is two panels. */
  actions?: ReactNode;
  children: ReactNode;
  className?: string;
}) => (
  <section
    data-testid={`apps-console-panel-${id}`}
    className={cn("flex min-w-0 flex-col rounded-lg border border-border/60 bg-card", className)}
  >
    <header className='flex min-h-10 items-center justify-between gap-3 border-border/60 border-b px-3 py-2'>
      <div className='flex min-w-0 items-center gap-1.5'>
        <h3 className='truncate font-semibold text-sm'>{title}</h3>
        {info ? <AdminInfo data-testid={`apps-console-panel-${id}-info`}>{info}</AdminInfo> : null}
        {scope ? (
          <span
            className={cn(
              "ml-0.5 shrink-0 rounded-sm px-1 py-px font-medium text-[10px]",
              scope.warn
                ? cn(ADMIN_TONE.warn.bg, ADMIN_TONE.warn.text)
                : "bg-muted text-muted-foreground"
            )}
            data-testid={`apps-console-panel-${id}-scope`}
          >
            {scope.label}
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
