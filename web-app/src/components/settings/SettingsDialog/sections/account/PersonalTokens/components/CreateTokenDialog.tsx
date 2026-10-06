import type React from "react";
import { useEffect, useReducer, useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { useCreateUserToken } from "@/hooks/api/userTokens/useUserTokenMutations";
import { useTokenOptions } from "@/hooks/api/userTokens/useUserTokens";
import type { TokenWithSecret } from "@/types/apiToken";
import {
  accessInputFromDraft,
  accessReducer,
  draftProblem,
  emptyDraft,
  lifetimeCap
} from "../accessDraft";
import {
  DEFAULT_EXPIRY_CHOICE,
  type ExpiryChoice,
  expiryInput,
  expiryProblem,
  fitChoiceToCap
} from "../expiry";
import AccessFields from "./AccessFields";
import ExpiryField from "./ExpiryField";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Handed the one response that carries the secret. */
  onCreated: (created: TokenWithSecret) => void;
}

// Until the options load, nobody is known to hold standing, so no flag is sent.
const NO_STANDING = { can_platform: false, can_partner: false };

/** Name, expiry and access for a new personal access token. All access is the default. */
const CreateTokenDialog: React.FC<Props> = ({ open, onOpenChange, onCreated }) => {
  const [name, setName] = useState("");
  const [picked, setPicked] = useState<ExpiryChoice>(DEFAULT_EXPIRY_CHOICE);
  const [draft, dispatch] = useReducer(accessReducer, undefined, emptyDraft);
  const options = useTokenOptions(open);
  const create = useCreateUserToken();

  // A fresh form each time it opens: the last token's name and picks don't carry over.
  useEffect(() => {
    if (!open) return;
    setName("");
    setPicked(DEFAULT_EXPIRY_CHOICE);
    dispatch({ type: "reset", draft: emptyDraft() });
  }, [open]);

  // Narrowing to an org with a max lifetime moves the expiry inside it, and widening again
  // brings the person's own choice back.
  const cap = lifetimeCap(draft, options.data);
  const expiry = fitChoiceToCap(picked, cap);
  const expiryBody = expiryInput(expiry);

  const canSubmit =
    name.trim().length > 0 &&
    !!expiryBody &&
    expiryProblem(expiry, cap) === null &&
    draftProblem(draft) === null &&
    !create.isPending;

  const handleSubmit = (event: React.FormEvent) => {
    event.preventDefault();
    if (!canSubmit || !expiryBody) return;
    create.mutate(
      {
        name: name.trim(),
        ...accessInputFromDraft(draft, options.data ?? NO_STANDING),
        ...expiryBody
      },
      {
        onSuccess: (created) => {
          onOpenChange(false);
          onCreated(created);
        }
      }
    );
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className='max-h-dvh overflow-y-auto sm:max-w-xl'
        data-testid='account-token-dialog'
      >
        <DialogHeader>
          <DialogTitle>Create a personal access token</DialogTitle>
          <DialogDescription className='text-xs'>
            A token signs in as you from oxyc, a script or CI. It can never do more than you can.
          </DialogDescription>
        </DialogHeader>

        {/* `min-w-0`: a grid item's default min-width lets a wide child push the form past the dialog. */}
        <form onSubmit={handleSubmit} className='flex min-w-0 flex-col gap-5'>
          <div className='flex flex-col gap-2'>
            <Label htmlFor='account-token-name'>Name</Label>
            <Input
              id='account-token-name'
              data-testid='account-token-name'
              placeholder='Where it will be used, e.g. laptop or deploy script'
              autoComplete='off'
              autoFocus
              value={name}
              onChange={(event) => setName(event.target.value)}
            />
          </div>

          <ExpiryField choice={expiry} onChange={setPicked} cap={cap} />

          <AccessFields
            draft={draft}
            dispatch={dispatch}
            options={options.data}
            optionsLoading={options.isLoading}
            optionsFailed={options.isError}
            onRetryOptions={() => options.refetch()}
          />

          <DialogFooter>
            <Button type='button' variant='outline' size='sm' onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button
              type='submit'
              size='sm'
              disabled={!canSubmit}
              data-testid='account-token-submit'
            >
              {create.isPending && <Spinner className='size-3' />}
              Create token
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
};

export default CreateTokenDialog;
