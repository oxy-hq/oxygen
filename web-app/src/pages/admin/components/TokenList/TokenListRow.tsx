import type { ReactNode } from "react";
import { TableRow } from "@/components/ui/shadcn/table";
import { cn } from "@/libs/shadcn/utils";
import type { Token } from "@/types/apiToken";
import { tokenState } from "./tokenState";

interface Props {
  /** `admin-<area>`: the row is `<area>-row`. */
  area: string;
  token: Token;
  /** More facts about the token for a selector to read, as `data-token-…` attributes. */
  facts?: Record<`data-token-${string}`, string>;
  children: ReactNode;
}

/**
 * One token's row in a staff list. It names the token and its state in the DOM
 * (`data-token-name`, `data-token-status`), and sets a token that no longer works back.
 */
export function TokenListRow({ area, token, facts, children }: Props) {
  const state = tokenState(token);
  return (
    <TableRow
      className={cn("border-border/50", state !== "active" && "text-muted-foreground")}
      data-testid={`${area}-row`}
      data-token-id={token.id}
      data-token-name={token.name}
      data-token-status={state}
      {...facts}
    >
      {children}
    </TableRow>
  );
}
