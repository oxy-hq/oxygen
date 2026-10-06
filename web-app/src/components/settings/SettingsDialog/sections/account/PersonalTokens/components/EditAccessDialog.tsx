import type React from "react";
import { useEffect, useReducer, useRef } from "react";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { useUpdateUserToken } from "@/hooks/api/userTokens/useUserTokenMutations";
import { useTokenOptions } from "@/hooks/api/userTokens/useUserTokens";
import type { Token } from "@/types/apiToken";
import {
  accessInputFromDraft,
  accessReducer,
  draftFromToken,
  draftProblem,
  lifetimeCap
} from "../accessDraft";
import { DAY_MS } from "../expiry";
import AccessFields from "./AccessFields";

interface Props {
  token: Token;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/**
 * The token outlives the tightest max lifetime among the orgs now picked. Editing access can't
 * change the expiry, so this is said rather than blocked: the org decides what it does about it.
 */
const outlivesCap = (token: Pick<Token, "expires_at">, capDays: number): boolean =>
  token.expires_at === null || new Date(token.expires_at).getTime() > Date.now() + capDays * DAY_MS;

/** Change what an existing token can reach. The secret and the expiry stay as they are. */
const EditAccessDialog: React.FC<Props> = ({ token, open, onOpenChange }) => {
  const [draft, dispatch] = useReducer(accessReducer, token, draftFromToken);
  const options = useTokenOptions(open);
  const update = useUpdateUserToken();

  // Start from the token as it is now each time the dialog opens, not from an abandoned edit.
  // Only on the closed → open edge: a list refetch hands down a new `token` object while the
  // dialog is open, and resetting on that would throw away the edit in progress.
  const wasOpen = useRef(false);
  useEffect(() => {
    if (open && !wasOpen.current) dispatch({ type: "reset", draft: draftFromToken(token) });
    wasOpen.current = open;
  }, [open, token]);

  const cap = lifetimeCap(draft, options.data);
  // Without the options the caller's standing is unknown, and saving could drop a flag by accident.
  const canSubmit = !!options.data && draftProblem(draft) === null && !update.isPending;

  const handleSubmit = (event: React.FormEvent) => {
    event.preventDefault();
    if (!canSubmit || !options.data) return;
    update.mutate(
      { id: token.id, request: accessInputFromDraft(draft, options.data) },
      { onSuccess: () => onOpenChange(false) }
    );
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className='max-h-dvh overflow-y-auto sm:max-w-xl'
        data-testid='account-token-edit-dialog'
      >
        <DialogHeader>
          <DialogTitle className='truncate pr-6'>Edit access for {token.name}</DialogTitle>
          <DialogDescription className='text-xs'>
            The secret stays the same, so nothing needs redeploying. The change applies to the
            token's next request.
          </DialogDescription>
        </DialogHeader>

        <form onSubmit={handleSubmit} className='flex min-w-0 flex-col gap-5'>
          <AccessFields
            draft={draft}
            dispatch={dispatch}
            options={options.data}
            optionsLoading={options.isLoading}
            optionsFailed={options.isError}
            onRetryOptions={() => options.refetch()}
          />

          {cap && outlivesCap(token, cap.days) && (
            <p className='text-muted-foreground text-xs' data-testid='account-token-edit-cap-note'>
              {cap.orgName} limits tokens to {cap.days} day{cap.days === 1 ? "" : "s"}, and this
              token lasts longer. {cap.orgName} may refuse it until its expiry is shortened.
            </p>
          )}

          <DialogFooter>
            <Button type='button' variant='outline' size='sm' onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button
              type='submit'
              size='sm'
              disabled={!canSubmit}
              data-testid='account-token-edit-submit'
            >
              {update.isPending && <Spinner className='size-3' />}
              Save access
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
};

export default EditAccessDialog;
