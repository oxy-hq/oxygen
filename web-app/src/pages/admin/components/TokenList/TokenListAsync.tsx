import { ShieldOff } from "lucide-react";
import type { ReactNode } from "react";
import { apiStatus } from "@/libs/apiError";
import type { Token } from "@/types/apiToken";
import { AdminAsync, type AdminAsyncQuery } from "../AdminAsync";
import { AdminEmptyState } from "../AdminEmptyState";

interface Props {
  /** `admin-<area>`: the refusal is `<area>-refused`. */
  area: string;
  /** The whole query, so a failed fetch and a list with nothing in it never read the same. */
  query: AdminAsyncQuery<Token[]>;
  /** What failed to load, in "Couldn't load …". */
  noun: string;
  /** What the viewer's access lacks, and where their own tokens are. */
  refused: string;
  /** Shown when the server listed no token at all. */
  empty: ReactNode;
  children: (tokens: Token[]) => ReactNode;
}

/**
 * Loading, failed, refused or empty around a staff token list. A 403 is the capability gate:
 * the rail hides the page from a viewer without it, so this is for one whose access changed
 * after the page loaded. Asking again cannot change it, so the refusal offers no Retry.
 */
export function TokenListAsync({ area, query, noun, refused, empty, children }: Props) {
  if (apiStatus(query.error) === 403) {
    return (
      <AdminEmptyState
        icon={ShieldOff}
        title="Your staff access doesn't include this list."
        description={refused}
        data-testid={`${area}-refused`}
      />
    );
  }
  return (
    <AdminAsync
      query={query}
      noun={noun}
      rows={4}
      isEmpty={(tokens) => tokens.length === 0}
      empty={empty}
    >
      {children}
    </AdminAsync>
  );
}
