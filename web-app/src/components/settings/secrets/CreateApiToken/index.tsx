import { KeySquare } from "lucide-react";
import { useId, useMemo, useState } from "react";
import type { LifetimeCap } from "@/components/settings/SettingsDialog/sections/account/PersonalTokens/accessDraft";
import ExpiryField from "@/components/settings/SettingsDialog/sections/account/PersonalTokens/components/ExpiryField";
import {
  DEFAULT_EXPIRY_CHOICE,
  type ExpiryChoice,
  expiryInput,
  expiryProblem,
  fitChoiceToCap
} from "@/components/settings/SettingsDialog/sections/account/PersonalTokens/expiry";
import { describeApiError } from "@/components/settings/SettingsDialog/sections/organization/ApiAccess/utils/errors";
import {
  serviceAccountNameError,
  toSlugInput
} from "@/components/settings/SettingsDialog/sections/organization/ApiAccess/utils/slug";
import { SecretInput } from "@/components/ui/SecretInput";
import { Button } from "@/components/ui/shadcn/button";
import { FieldError } from "@/components/ui/shadcn/field";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { useAuth } from "@/contexts/AuthContext";
import { useServiceAccounts, useTokenPolicy } from "@/hooks/api/orgApiAccess";
import { useRole } from "@/hooks/useRole";
import useCurrentOrg from "@/stores/useCurrentOrg";
import type { ServiceAccountRole } from "@/types/orgApiAccess";
import {
  type AccountChoice,
  type MintedApiToken,
  suggestedAccountName,
  tokenNameFor
} from "./mint";
import type { MintedTokenState } from "./useMintedApiToken";

const NEW_ACCOUNT = "__new__";

const ROLE_LABELS: Record<ServiceAccountRole, string> = { member: "Member", admin: "Admin" };

export interface SecretAppRef {
  name: string;
  slug: string;
}

interface SecretValueFieldProps {
  id: string;
  label: string;
  placeholder: string;
  value: string;
  onChange: (value: string) => void;
  error?: string;
  token: MintedTokenState;
  /** The secret's name as typed so far: the new token is named after it. */
  secretName: string;
  /** Set for one app's own secret: the account is suggested after the app. */
  app?: SecretAppRef;
}

/**
 * A secret's value: typed or pasted, or — for a secret that holds an Oxygen API token, the way a
 * custom app that calls Oxygen keeps one — created here and saved straight into the secret.
 *
 * This is the shortcut that replaces the INC-180 round trip: create a service account and a token
 * under API tokens, copy the token, find the app's secret, paste it. The token is never shown:
 * it goes from the mint response into the save, and if the form closes without saving it is
 * revoked (`useMintedApiToken`).
 */
export function SecretValueField({
  id,
  label,
  placeholder,
  value,
  onChange,
  error,
  token,
  secretName,
  app
}: SecretValueFieldProps) {
  return (
    <div className='grid gap-2'>
      <Label htmlFor={id}>{label}</Label>
      {token.minted ? (
        <MintedNote minted={token.minted} onUndo={token.discard} />
      ) : (
        <>
          <SecretInput
            id={id}
            placeholder={placeholder}
            value={value}
            onChange={(e) => onChange(e.target.value)}
            className={error ? "border-destructive" : ""}
          />
          {error && <FieldError>{error}</FieldError>}
          <CreateApiTokenControl token={token} secretName={secretName} app={app} />
        </>
      )}
    </div>
  );
}

const formatExpiry = (expiresAt: string | null): string =>
  expiresAt
    ? `expires ${new Date(expiresAt).toLocaleDateString(undefined, { dateStyle: "medium" })}`
    : "never expires";

function MintedNote({ minted, onUndo }: { minted: MintedApiToken; onUndo: () => void }) {
  return (
    <div
      className='flex items-start gap-2 rounded-md border bg-muted/40 p-3 text-xs'
      data-testid='secret-minted-token'
    >
      <KeySquare className='mt-0.5 size-4 shrink-0 text-muted-foreground' aria-hidden />
      <div className='min-w-0 flex-1'>
        <p className='font-medium'>
          A new API token for service account{" "}
          <span className='font-mono'>{minted.account.name}</span>
        </p>
        <p className='text-muted-foreground'>
          {ROLE_LABELS[minted.account.role]} access, {formatExpiry(minted.expiresAt)}. It is saved
          into this secret when you save, and never shown.
        </p>
      </div>
      <Button
        type='button'
        variant='ghost'
        size='sm'
        onClick={onUndo}
        data-testid='secret-minted-token-undo'
      >
        Undo
      </Button>
    </div>
  );
}

function CreateApiTokenControl({
  token,
  secretName,
  app
}: {
  token: MintedTokenState;
  secretName: string;
  app?: SecretAppRef;
}) {
  const { isLocalMode } = useAuth();
  const { is } = useRole();
  const orgId = useCurrentOrg((s) => s.org?.id);
  const [open, setOpen] = useState(false);

  // Local mode has no sign-in and no service accounts to mint on.
  if (isLocalMode || !orgId) return null;
  // Service-account routes take an org owner or admin in a browser session.
  if (!is.orgAdmin) {
    return (
      <p className='text-muted-foreground text-xs' data-testid='secret-api-token-admin-only'>
        Holds an Oxygen API token? An organization admin can create one here, straight into this
        secret.
      </p>
    );
  }
  if (!open) {
    return (
      <div className='flex flex-wrap items-center gap-x-2 text-xs'>
        <span className='text-muted-foreground'>Holds an Oxygen API token?</span>
        <Button
          type='button'
          variant='link'
          size='sm'
          className='h-auto p-0 text-xs'
          onClick={() => setOpen(true)}
          data-testid='secret-create-api-token'
        >
          Create API token
        </Button>
      </div>
    );
  }
  return (
    <MintForm
      orgId={orgId}
      token={token}
      secretName={secretName}
      app={app}
      onCancel={() => setOpen(false)}
    />
  );
}

function MintForm({
  orgId,
  token,
  secretName,
  app,
  onCancel
}: {
  orgId: string;
  token: MintedTokenState;
  secretName: string;
  app?: SecretAppRef;
  onCancel: () => void;
}) {
  const accountSelectId = useId();
  const nameId = useId();
  const roleId = useId();
  const accounts = useServiceAccounts(orgId);
  const policy = useTokenPolicy(orgId);

  const usable = useMemo(
    () => (accounts.data ?? []).filter((account) => !account.disabled_at),
    [accounts.data]
  );
  const suggested = suggestedAccountName(secretName, app?.slug);
  // An account already named after the app is the one to reuse: one account per app.
  const existingMatch = usable.find((account) => account.name === suggested);

  const [picked, setPicked] = useState<string | null>(null);
  const accountValue = picked ?? existingMatch?.id ?? NEW_ACCOUNT;
  const [name, setName] = useState<string | null>(null);
  const newName = name ?? suggested;
  const [role, setRole] = useState<ServiceAccountRole>("member");

  const maxDays = policy.data?.max_lifetime_days ?? null;
  const cap = useMemo<LifetimeCap | null>(
    () => (maxDays === null ? null : { days: maxDays, orgName: "This organization" }),
    [maxDays]
  );
  const [expiry, setExpiry] = useState<ExpiryChoice | null>(null);
  const expiryChoice = expiry ?? fitChoiceToCap(DEFAULT_EXPIRY_CHOICE, cap);

  const [touched, setTouched] = useState(false);
  const [serverError, setServerError] = useState<string | null>(null);

  const isNew = accountValue === NEW_ACCOUNT;
  const nameError = isNew
    ? serviceAccountNameError(
        newName,
        (accounts.data ?? []).map((account) => account.name)
      )
    : null;
  const expiryBody = expiryInput(expiryChoice);
  const expiryError = expiryProblem(expiryChoice, cap) ?? (expiryBody ? null : "Pick a date.");
  const secretNameError = secretName.trim() ? null : "Name the secret first.";

  const submit = async () => {
    setTouched(true);
    if (nameError || expiryError || secretNameError || !expiryBody || token.pending) return;
    const account: AccountChoice | null = isNew
      ? { kind: "new", name: newName, role }
      : (() => {
          const found = usable.find((candidate) => candidate.id === accountValue);
          return found ? { kind: "existing", account: found } : null;
        })();
    if (!account) return;
    setServerError(null);
    try {
      await token.mint({
        orgId,
        account,
        tokenName: tokenNameFor(secretName.trim(), app?.name),
        expiry: expiryBody
      });
    } catch (err) {
      setServerError(describeApiError(err, "Couldn't create the token."));
    }
  };

  return (
    <div className='flex flex-col gap-3 rounded-md border p-3' data-testid='secret-api-token-form'>
      <p className='text-muted-foreground text-xs'>
        Creates a service account token and saves it into this secret when you save. It is listed
        under API tokens → Service accounts, where it can be revoked.
      </p>

      <div className='grid gap-1.5'>
        <Label htmlFor={accountSelectId} className='text-xs'>
          Service account
        </Label>
        <Select value={accountValue} onValueChange={setPicked}>
          <SelectTrigger
            id={accountSelectId}
            size='sm'
            className='text-xs'
            data-testid='secret-api-token-account'
          >
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={NEW_ACCOUNT} className='text-xs'>
              New service account
            </SelectItem>
            {usable.map((account) => (
              <SelectItem key={account.id} value={account.id} className='text-xs'>
                {account.name} ({ROLE_LABELS[account.org_role]})
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>

      {isNew && (
        <div className='grid gap-3 sm:grid-cols-[1fr_auto]'>
          <div className='grid gap-1.5'>
            <Label htmlFor={nameId} className='text-xs'>
              Name
            </Label>
            <Input
              id={nameId}
              value={newName}
              onChange={(e) => setName(toSlugInput(e.target.value))}
              className='text-xs'
              autoComplete='off'
              aria-invalid={touched && !!nameError}
              data-testid='secret-api-token-account-name'
            />
            {touched && nameError && <FieldError>{nameError}</FieldError>}
          </div>
          <div className='grid gap-1.5'>
            <Label htmlFor={roleId} className='text-xs'>
              Access
            </Label>
            <Select value={role} onValueChange={(value) => setRole(value as ServiceAccountRole)}>
              <SelectTrigger
                id={roleId}
                size='sm'
                className='w-32 text-xs'
                data-testid='secret-api-token-role'
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value='member' className='text-xs'>
                  Member
                </SelectItem>
                <SelectItem value='admin' className='text-xs'>
                  Admin
                </SelectItem>
              </SelectContent>
            </Select>
          </div>
          <p className='text-muted-foreground text-xs sm:col-span-2'>
            Admin can do what an organization admin can, such as manage tablets. Choose it only if
            the app needs it.
          </p>
        </div>
      )}

      <ExpiryField
        choice={expiryChoice}
        onChange={setExpiry}
        cap={cap}
        testId='secret-api-token-expiry'
        labelClassName='text-xs'
      />

      {touched && (secretNameError || (!expiryBody && expiryError)) && (
        <FieldError>{secretNameError ?? expiryError}</FieldError>
      )}
      {serverError && (
        <p className='text-destructive text-xs' role='alert' data-testid='secret-api-token-error'>
          {serverError}
        </p>
      )}

      <div className='flex justify-end gap-2'>
        <Button type='button' variant='outline' size='sm' onClick={onCancel}>
          Cancel
        </Button>
        <Button
          type='button'
          size='sm'
          onClick={submit}
          disabled={token.pending}
          data-testid='secret-api-token-submit'
        >
          {token.pending ? "Creating..." : "Create token"}
        </Button>
      </div>
    </div>
  );
}
