import { type FormEvent, useMemo, useState } from "react";
import type { LifetimeCap } from "@/components/settings/SettingsDialog/sections/account/PersonalTokens/accessDraft";
import ExpiryField from "@/components/settings/SettingsDialog/sections/account/PersonalTokens/components/ExpiryField";
import {
  DEFAULT_EXPIRY_CHOICE,
  type ExpiryChoice,
  expiryInput,
  expiryProblem,
  fitChoiceToCap
} from "@/components/settings/SettingsDialog/sections/account/PersonalTokens/expiry";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import { Input } from "@/components/ui/shadcn/input";
import { useCreateServiceAccountToken } from "@/hooks/api/orgApiAccess";
import type { TokenWithSecret } from "@/types/apiToken";
import type { ServiceAccount } from "@/types/orgApiAccess";
import { AccessPicker } from "../../../shared/AccessPicker";
import { FormField } from "../../../shared/FormField";
import { usePickableWorkspaces } from "../../../shared/usePickableLists";
import { describeApiError } from "../../../utils/errors";
import {
  type AccessDraft,
  accessDraftError,
  buildGrantInputs,
  ORG_WIDE_ACCESS
} from "../../../utils/grants";

/** Long enough for "nightly warehouse sync (prod)", short enough for a table cell. */
const TOKEN_NAME_MAX = 100;

interface CreateTokenDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  orgId: string;
  account: ServiceAccount;
  maxLifetimeDays: number | null;
  /** Receives the token and its secret; the secret is shown once by the caller. */
  onCreated: (secret: TokenWithSecret) => void;
}

export function CreateTokenDialog({
  open,
  onOpenChange,
  orgId,
  account,
  maxLifetimeDays,
  onCreated
}: CreateTokenDialogProps) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className='max-h-[90vh] overflow-y-auto sm:max-w-md'
        data-testid='api-access-token-dialog'
      >
        <DialogHeader>
          <DialogTitle className='text-base'>Create a token for {account.name}</DialogTitle>
          <DialogDescription className='text-xs'>
            The token signs in as this account. You'll see its secret once, right after creating it.
          </DialogDescription>
        </DialogHeader>
        {/* Mounted per open, so the form always starts clean. */}
        {open && (
          <CreateTokenForm
            orgId={orgId}
            account={account}
            maxLifetimeDays={maxLifetimeDays}
            onCancel={() => onOpenChange(false)}
            onCreated={(secret) => {
              onOpenChange(false);
              onCreated(secret);
            }}
          />
        )}
      </DialogContent>
    </Dialog>
  );
}

function CreateTokenForm({
  orgId,
  account,
  maxLifetimeDays,
  onCancel,
  onCreated
}: {
  orgId: string;
  account: ServiceAccount;
  maxLifetimeDays: number | null;
  onCancel: () => void;
  onCreated: (secret: TokenWithSecret) => void;
}) {
  const create = useCreateServiceAccountToken();
  const workspaces = usePickableWorkspaces(orgId);
  const [name, setName] = useState("");
  // The org's own cap, in the shape the shared expiry field reads.
  const cap = useMemo<LifetimeCap | null>(
    () =>
      maxLifetimeDays === null ? null : { days: maxLifetimeDays, orgName: "This organization" },
    [maxLifetimeDays]
  );
  // 90 days, or the org's cap when that is shorter.
  const [expiry, setExpiry] = useState<ExpiryChoice>(() =>
    fitChoiceToCap(DEFAULT_EXPIRY_CHOICE, cap)
  );
  const [access, setAccess] = useState<AccessDraft>(ORG_WIDE_ACCESS);
  const [touched, setTouched] = useState(false);
  const [serverError, setServerError] = useState<string | null>(null);

  const nameError = name.trim() ? null : "Give the token a name you'll recognise later.";
  const expiryBody = expiryInput(expiry);
  // The field itself says when a choice breaks the cap; a date not picked yet is said on submit.
  const expiryError = expiryProblem(expiry, cap) ?? (expiryBody ? null : "Pick a date.");
  const accessError = accessDraftError(access);

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    setTouched(true);
    if (nameError || expiryError || accessError || !expiryBody || create.isPending) return;
    setServerError(null);
    try {
      const grants = buildGrantInputs(access, account.org_role);
      const secret = await create.mutateAsync({
        orgId,
        saId: account.id,
        request: { name: name.trim(), ...expiryBody, ...(grants ? { grants } : {}) }
      });
      onCreated(secret);
    } catch (err) {
      setServerError(describeApiError(err, "Couldn't create the token."));
    }
  };

  return (
    <form onSubmit={handleSubmit} className='flex min-w-0 flex-col gap-4 pt-1' noValidate>
      <FormField
        htmlFor='api-access-token-name'
        label='Name'
        error={touched ? nameError : null}
        hint='Say where it will live, like "nightly sync on the ops server".'
      >
        <Input
          id='api-access-token-name'
          value={name}
          onChange={(e) => setName(e.target.value)}
          maxLength={TOKEN_NAME_MAX}
          placeholder='nightly sync'
          className='text-xs'
          autoComplete='off'
          autoFocus
          aria-invalid={touched && !!nameError}
          data-testid='api-access-token-name'
        />
      </FormField>

      <div className='flex min-w-0 flex-col gap-1.5'>
        <ExpiryField
          choice={expiry}
          onChange={setExpiry}
          cap={cap}
          testId='api-access-token-expiry'
          labelClassName='text-xs'
        />
        {touched && !expiryBody && (
          <p className='text-destructive text-xs' role='alert'>
            {expiryError}
          </p>
        )}
      </div>

      <FormField label='Access' error={touched ? accessError : null}>
        <AccessPicker
          value={access}
          onChange={setAccess}
          accountRole={account.org_role}
          workspaces={workspaces}
          testId='api-access-token-access'
        />
      </FormField>

      {serverError && (
        <p className='text-destructive text-xs' role='alert' data-testid='api-access-token-error'>
          {serverError}
        </p>
      )}

      <div className='flex justify-end gap-2'>
        <Button type='button' variant='outline' size='sm' onClick={onCancel}>
          Cancel
        </Button>
        <Button
          type='submit'
          size='sm'
          disabled={create.isPending}
          data-testid='api-access-token-submit'
        >
          {create.isPending ? "Creating..." : "Create token"}
        </Button>
      </div>
    </form>
  );
}
