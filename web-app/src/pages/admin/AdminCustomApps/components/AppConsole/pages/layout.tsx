import type { ReactNode } from "react";

/** The console's page grid: the main thing wide on the left, its reference on the right. */
export const TwoColumns = ({ children }: { children: ReactNode }) => (
  <div className='grid items-start gap-3 xl:grid-cols-[minmax(0,1.35fr)_minmax(0,1fr)]'>
    {children}
  </div>
);

export const Column = ({ children }: { children: ReactNode }) => (
  <div className='flex min-w-0 flex-col gap-3'>{children}</div>
);

/** Which environment a page's panels show, as the console's switch names it. */
export type Scope = { label: string; warn?: boolean };

export const PRODUCTION_ONLY: Scope = { label: "Production" };
