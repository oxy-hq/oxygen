import type { ReactNode } from "react";

/**
 * The console's page grid: the main thing wide on the left, its reference on the right.
 *
 * Split by the console's own width, never the window's. Beside the column browser a
 * 1440px window leaves the console about 670px, and splitting that cut the right column
 * (where the Checks panel lives) to 260px. Now it stacks below 64rem of console and
 * splits only where each column has room.
 */
export const TwoColumns = ({ children }: { children: ReactNode }) => (
  <div className='@container' data-testid='console-two-columns'>
    <div className='grid items-start gap-3 @5xl:grid-cols-[minmax(0,1.35fr)_minmax(0,1fr)]'>
      {children}
    </div>
  </div>
);

export const Column = ({ children }: { children: ReactNode }) => (
  <div className='flex min-w-0 flex-col gap-3'>{children}</div>
);

/** Which environment a page's panels show, as the console's switch names it. */
export type Scope = { label: string; warn?: boolean };

export const PRODUCTION_ONLY: Scope = { label: "Production" };
