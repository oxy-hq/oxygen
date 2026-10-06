import type React from "react";
import { useEffect, useReducer, useState } from "react";
import { KeyHint, SubmitChordHint } from "@/components/ui/KeyHint";
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
import { Spinner } from "@/components/ui/shadcn/spinner";
import { useCreateUserToken } from "@/hooks/api/userTokens/useUserTokenMutations";
import { useTokenOptions } from "@/hooks/api/userTokens/useUserTokens";
import { isSubmitChord, submitChordShortcut } from "@/libs/submitChord";
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
import useSandboxAgentForm from "../useSandboxAgentForm";
import AccessFields from "./AccessFields";
import ExpiryField from "./ExpiryField";
import SandboxAgentFields from "./SandboxAgentFields";
import SandboxSummary from "./SandboxAgentFields/components/SandboxSummary";
import SandboxLifetimeField from "./SandboxLifetimeField";
import SheetRow from "./SheetRow";
import { TOKEN_NAME_MAX } from "./TokenRow/components/TokenName";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Handed the one response that carries the secret. */
  onCreated: (created: TokenWithSecret) => void;
}

// Until the options load, nobody is known to hold standing, so no flag is sent.
const NO_STANDING = { can_platform: false, can_partner: false };

/**
 * Name, access and expiry for a new personal access token, as one sheet: each label in a gutter
 * and each control on the same edge. All access is the default.
 *
 * Someone who may mint a sandbox agent token is offered it as a third type beside the two access
 * modes. Choosing it swaps the expiry for a lifetime in hours and the workspaces for apps, and
 * sends a different body: the name is the only field the two share.
 *
 * Command+Enter (Control+Enter off an Apple keyboard) creates the token from any field.
 *
 * The dialog is never taller than the window. Its title and its buttons stay put, and the fields
 * between them give way: first the list of apps, which scrolls inside its row, and only then the
 * fields as a whole.
 */
const CreateTokenDialog: React.FC<Props> = ({ open, onOpenChange, onCreated }) => {
  const [name, setName] = useState("");
  const [picked, setPicked] = useState<ExpiryChoice>(DEFAULT_EXPIRY_CHOICE);
  const [draft, dispatch] = useReducer(accessReducer, undefined, emptyDraft);
  const options = useTokenOptions(open);
  const create = useCreateUserToken();
  const sandbox = useSandboxAgentForm(open, name, options.data);

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

  const canSubmitPersonal =
    name.trim().length > 0 &&
    !!expiryBody &&
    expiryProblem(expiry, cap) === null &&
    draftProblem(draft) === null &&
    !create.isPending;
  const canSubmit = sandbox.active ? sandbox.canSubmit : canSubmitPersonal;
  const isPending = sandbox.active ? sandbox.isPending : create.isPending;

  const showSecret = (created: TokenWithSecret) => {
    onOpenChange(false);
    onCreated(created);
  };

  const submit = () => {
    if (sandbox.active) return sandbox.submit(showSecret);
    if (!canSubmitPersonal || !expiryBody) return;
    create.mutate(
      {
        name: name.trim(),
        ...accessInputFromDraft(draft, options.data ?? NO_STANDING),
        ...expiryBody
      },
      { onSuccess: showSecret }
    );
  };

  const handleSubmit = (event: React.FormEvent) => {
    event.preventDefault();
    submit();
  };

  const handleKeyDown = (event: React.KeyboardEvent) => {
    if (!isSubmitChord(event.nativeEvent)) return;
    event.preventDefault();
    submit();
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className='flex max-h-[calc(100dvh-2rem)] flex-col gap-5 sm:max-w-160'
        data-testid='account-token-dialog'
      >
        <DialogHeader className='shrink-0 gap-1 pr-6'>
          <DialogTitle className='text-base leading-6'>Create a personal access token</DialogTitle>
          <DialogDescription>
            A token signs in as you from oxyc, a script or CI. It can never do more than you can.
          </DialogDescription>
        </DialogHeader>

        {/* `onKeyDown`: the submit chord, heard from every field. */}
        <form
          onSubmit={handleSubmit}
          onKeyDown={handleKeyDown}
          className='flex min-h-0 min-w-0 flex-col gap-5 text-sm'
        >
          {/* The fields scroll here if they must, with room left for a focus ring at the edge. */}
          <div
            className='-m-1 flex min-h-0 flex-col gap-3 overflow-y-auto p-1'
            data-testid='account-token-fields'
          >
            <SheetRow label='Name' htmlFor='account-token-name'>
              <Input
                id='account-token-name'
                data-testid='account-token-name'
                placeholder={
                  sandbox.active
                    ? "What the agent is working on, e.g. store ops refunds"
                    : "Where it will be used, e.g. laptop or deploy script"
                }
                autoComplete='off'
                autoFocus
                // The server's limit for either type. Past it a sandbox agent mint answers 400
                // `invalid_sandbox_token`, the code a wrong app count or lifetime gets too.
                maxLength={TOKEN_NAME_MAX}
                value={name}
                onChange={(event) => setName(event.target.value)}
              />
            </SheetRow>

            <AccessFields
              draft={draft}
              dispatch={dispatch}
              options={options.data}
              optionsLoading={options.isLoading}
              optionsFailed={options.isError}
              onRetryOptions={() => options.refetch()}
              sandbox={
                sandbox.offered
                  ? {
                      active: sandbox.active,
                      onActiveChange: sandbox.setActive,
                      fields: (
                        <SandboxAgentFields
                          apps={sandbox.apps}
                          picked={sandbox.picked}
                          limits={sandbox.limits}
                          refusal={sandbox.refusal}
                          toggle={sandbox.toggle}
                        />
                      )
                    }
                  : undefined
              }
            />

            {sandbox.active ? (
              <>
                <SandboxLifetimeField
                  choice={sandbox.lifetime}
                  onChange={sandbox.setLifetime}
                  limits={sandbox.limits}
                />
                <SandboxSummary />
              </>
            ) : (
              <ExpiryField choice={expiry} onChange={setPicked} cap={cap} variant='sheet' />
            )}
          </div>

          <DialogFooter className='shrink-0'>
            <Button
              type='button'
              variant='outline'
              className='px-3'
              onClick={() => onOpenChange(false)}
              aria-keyshortcuts='Escape'
              data-testid='account-token-cancel'
            >
              Cancel
              <KeyHint className='-mr-1'>Esc</KeyHint>
            </Button>
            <Button
              type='submit'
              className='px-3'
              disabled={!canSubmit}
              aria-keyshortcuts={submitChordShortcut()}
              data-testid='account-token-submit'
            >
              {isPending && <Spinner className='size-4' />}
              Create token
              {canSubmit && <SubmitChordHint tone='onPrimary' className='-mr-1' />}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
};

export default CreateTokenDialog;
