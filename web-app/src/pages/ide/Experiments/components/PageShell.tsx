import type { ReactNode } from "react";

/**
 * The full-width box an Experiments page state renders in. The IDE outlet is a
 * flex row, so a bare loading / empty / error state shrinks to its own content
 * and sits against the rail; this makes it take the page.
 */
export default function PageShell({ children }: { children: ReactNode }) {
  return (
    <div className='flex h-full w-full min-w-0 flex-1 flex-col' data-testid='experiments-page'>
      {children}
    </div>
  );
}
