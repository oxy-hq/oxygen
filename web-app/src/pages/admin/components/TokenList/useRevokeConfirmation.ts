import { useState } from "react";
import type { Token } from "@/types/apiToken";

export interface RevokeConfirmation {
  /** The token being asked about. Kept after the dialog closes, so its title does not blank. */
  target: Token | null;
  open: boolean;
  setOpen: (open: boolean) => void;
  /** A row's Revoke: nothing is revoked until the dialog is confirmed. */
  ask: (token: Token) => void;
  confirm: () => void;
}

/** The asking before a revoke: which token, whether the dialog is open, and the confirm. */
export const useRevokeConfirmation = (
  revoke: (token: Pick<Token, "id" | "name">) => void
): RevokeConfirmation => {
  const [target, setTarget] = useState<Token | null>(null);
  const [open, setOpen] = useState(false);

  return {
    target,
    open,
    setOpen,
    ask: (token) => {
      setTarget(token);
      setOpen(true);
    },
    confirm: () => {
      if (target) revoke({ id: target.id, name: target.name });
    }
  };
};
