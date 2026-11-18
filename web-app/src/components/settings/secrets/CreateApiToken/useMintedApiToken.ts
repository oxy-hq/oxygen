import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { invalidateApiAccess } from "@/hooks/api/orgApiAccess/shared";
import { discardMinted, type MintedApiToken, type MintRequest, mintApiToken } from "./mint";

export interface MintedTokenState {
  /** The token waiting to be saved into the secret, or `null`. */
  minted: MintedApiToken | null;
  pending: boolean;
  mint: (request: MintRequest) => Promise<void>;
  /**
   * Revoke a token that was not saved, and forget it. Also takes back one still being minted:
   * it is revoked the moment it arrives. Safe to call when there is none.
   */
  discard: () => void;
  /** The secret now holds the token: forget it without revoking. */
  settle: () => void;
}

/**
 * The "Create API token" state of one secret form. The form decides when the token is saved; this
 * makes sure a token that never reaches a secret does not outlive the form. Close the form without
 * saving and the token is revoked, with its account if the account was made for it.
 *
 * Minting is two requests (create the account, then the token), and the form can close while they
 * are in flight. So `discard` advances a generation even when nothing is minted yet, and a mint
 * that resolves under an older generation is revoked at once instead of kept: otherwise it would
 * be stored after the close, stay live, and turn up as the value of the next secret the dialog
 * opens on.
 */
export function useMintedApiToken(): MintedTokenState {
  const queryClient = useQueryClient();
  const [minted, setMinted] = useState<MintedApiToken | null>(null);
  const [pending, setPending] = useState(false);
  // Read by `discard`, which a dialog's close handler may call from a stale closure.
  const current = useRef<MintedApiToken | null>(null);
  // Bumped by every discard. A mint that finishes under an older value was abandoned.
  const generation = useRef(0);

  const keep = useCallback((next: MintedApiToken | null) => {
    current.current = next;
    setMinted(next);
  }, []);

  const takeBack = useCallback(
    (unsaved: MintedApiToken) => {
      discardMinted(unsaved)
        .catch(() =>
          toast.error(
            `Couldn't revoke the unused token on ${unsaved.account.name}. Revoke it under API tokens → Service accounts.`
          )
        )
        .finally(() => invalidateApiAccess(queryClient, unsaved.orgId));
    },
    [queryClient]
  );

  const mint = useCallback(
    async (request: MintRequest) => {
      const mine = generation.current;
      setPending(true);
      try {
        const result = await mintApiToken(request);
        if (generation.current !== mine) {
          // The form closed, or the token was undone, while this was in flight.
          takeBack(result);
          return;
        }
        keep(result);
      } finally {
        setPending(false);
        invalidateApiAccess(queryClient, request.orgId);
      }
    },
    [queryClient, keep, takeBack]
  );

  const discard = useCallback(() => {
    generation.current += 1;
    const unsaved = current.current;
    if (!unsaved) return;
    keep(null);
    takeBack(unsaved);
  }, [keep, takeBack]);

  const settle = useCallback(() => keep(null), [keep]);

  // A form that unmounts unsaved takes its token with it.
  useEffect(() => () => discard(), [discard]);

  return { minted, pending, mint, discard, settle };
}
