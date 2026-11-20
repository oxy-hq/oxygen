// "Try again" for a part of a screen whose read failed: mount that part again.
//
// Four apps define their own (INVENTORY section 5): the reference's tuple (oxy-hq/reference-app
// `src/hooks/useReread.ts`), kept verbatim by Locations and Watchlist, which add a context to
// reach it; Store Ops' `{ key, reread }`; and Bookkeeping's `RereadBoundary`
// (`components/ui/Reread.tsx`), which keys its children itself and hands the bump to any
// `LoadError` below it. Picked:
//
//   `useReread()`      the reference's `[attempt, reread]`, unchanged. Three of the five
//                      definitions have this shape, and the reference app moves onto the SDK by
//                      an import swap (decision 58).
//   `RereadBoundary`   Bookkeeping's form, as the better default for new code: nothing to thread,
//                      and a `LoadError` inside offers Try again without being handed one. It is
//                      what makes Try again re-read only what failed (decision 28): put the
//                      boundary around the part that reads, and a boundary nested inside another
//                      re-reads only itself.
//   `useRereadAction`  the nearest boundary's `reread` (Locations' name for the reader), or
//                      undefined outside one.
//
// The mechanism: each hook owns its own read, so a screen has no handle on any one of them; a
// fresh mount runs every read under the key from the start, as the first visit did. What it costs:
// local state under the key starts over too (a sort, an open row). Anything that must survive a
// re-read belongs above the boundary or in the URL.

import {
  createContext,
  Fragment,
  type ReactNode,
  useCallback,
  useContext,
  useReducer
} from "react";

/** The reference's form: key the reading part with `attempt`; `reread` mounts it afresh. */
export function useReread(): [attempt: number, reread: () => void] {
  const [attempt, reread] = useReducer((n: number) => n + 1, 0);
  return [attempt, reread];
}

const RereadContext = createContext<(() => void) | undefined>(undefined);

export interface RereadBoundaryProps {
  children: ReactNode;
}

/** Mounts its children again on `reread`, so every read below runs again. A `LoadError` inside
 *  offers it as its Try again unless handed its own. */
export function RereadBoundary({ children }: RereadBoundaryProps) {
  const [attempt, bump] = useReducer((n: number) => n + 1, 0);
  const reread = useCallback(() => bump(), []);
  return (
    <RereadContext.Provider value={reread}>
      <Fragment key={attempt}>{children}</Fragment>
    </RereadContext.Provider>
  );
}

/** The nearest `RereadBoundary`'s re-read, or undefined outside one. */
export function useRereadAction(): (() => void) | undefined {
  return useContext(RereadContext);
}
