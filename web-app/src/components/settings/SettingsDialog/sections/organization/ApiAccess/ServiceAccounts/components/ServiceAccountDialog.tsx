import { type FormEvent, useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import { Input } from "@/components/ui/shadcn/input";
import { Textarea } from "@/components/ui/shadcn/textarea";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/shadcn/toggle-group";
import { useCreateServiceAccount, useUpdateServiceAccount } from "@/hooks/api/orgApiAccess";
import type { ServiceAccount, ServiceAccountRole } from "@/types/orgApiAccess";
import { FormField } from "../../shared/FormField";
import { describeApiError, isApiErrorCode } from "../../utils/errors";
import { serviceAccountHandle, serviceAccountNameError, toSlugInput } from "../../utils/slug";

interface ServiceAccountDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  orgId: string;
  orgSlug: string;
  /** Set to edit that account; omit to create one. */
  account?: ServiceAccount | null;
  /** Names already in use, to catch a clash before the round trip. */
  takenNames: string[];
  onCreated?: (account: ServiceAccount) => void;
}

const ROLE_HINTS: Record<ServiceAccountRole, string> = {
  member:
    "What an organization member can do in the workspaces its tokens reach. The right choice for most automation.",
  admin:
    "Also manages workspace settings: databases, secrets, apps and keys. A service account can never be an owner."
};

export function ServiceAccountDialog({
  open,
  onOpenChange,
  orgId,
  orgSlug,
  account,
  takenNames,
  onCreated
}: ServiceAccountDialogProps) {
  const editing = !!account;
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className='sm:max-w-md' data-testid='api-access-account-dialog'>
        <DialogHeader>
          <DialogTitle className='text-base'>
            {editing ? `Edit ${account.name}` : "New service account"}
          </DialogTitle>
          <DialogDescription className='text-xs'>
            {editing
              ? "Its name can't change: workflows and scripts refer to the account by it."
              : "An identity for automation: CI, scripts, integrations. It has no email and can't sign in, and it doesn't take a seat."}
          </DialogDescription>
        </DialogHeader>
        {/* Keyed so reopening for another account starts from that account's values. */}
        <ServiceAccountForm
          key={account?.id ?? "new"}
          orgId={orgId}
          orgSlug={orgSlug}
          account={account ?? null}
          takenNames={takenNames}
          onDone={() => onOpenChange(false)}
          onCreated={onCreated}
        />
      </DialogContent>
    </Dialog>
  );
}

function ServiceAccountForm({
  orgId,
  orgSlug,
  account,
  takenNames,
  onDone,
  onCreated
}: {
  orgId: string;
  orgSlug: string;
  account: ServiceAccount | null;
  takenNames: string[];
  onDone: () => void;
  onCreated?: (account: ServiceAccount) => void;
}) {
  const create = useCreateServiceAccount();
  const update = useUpdateServiceAccount();
  const [name, setName] = useState(account?.name ?? "");
  const [description, setDescription] = useState(account?.description ?? "");
  const [role, setRole] = useState<ServiceAccountRole>(account?.org_role ?? "member");
  const [touched, setTouched] = useState(false);
  const [serverError, setServerError] = useState<string | null>(null);
  const [nameTaken, setNameTaken] = useState<string | null>(null);

  const isPending = create.isPending || update.isPending;
  const nameError = account
    ? null
    : (serviceAccountNameError(name, takenNames) ??
      (nameTaken === name ? "A service account with that name already exists here." : null));

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    setTouched(true);
    if (nameError || isPending) return;
    setServerError(null);
    const trimmed = description.trim();
    try {
      if (account) {
        await update.mutateAsync({
          orgId,
          saId: account.id,
          request: { description: trimmed || null, org_role: role }
        });
        toast.success(`Saved ${account.name}`);
      } else {
        const created = await create.mutateAsync({
          orgId,
          request: { name, org_role: role, ...(trimmed ? { description: trimmed } : {}) }
        });
        toast.success(`Created ${created.name}`);
        onCreated?.(created);
      }
      onDone();
    } catch (err) {
      // A clash the list didn't know about yet belongs on the name field.
      if (isApiErrorCode(err, "name_taken")) {
        setNameTaken(name);
        return;
      }
      setServerError(describeApiError(err, "Couldn't save the service account."));
    }
  };

  return (
    <form onSubmit={handleSubmit} className='flex min-w-0 flex-col gap-4 pt-1' noValidate>
      {!account && (
        <FormField
          htmlFor='api-access-account-name'
          label='Name'
          error={touched ? nameError : null}
          hint={
            name ? (
              <>
                Workflows will call it{" "}
                <code className='font-mono text-foreground'>
                  {serviceAccountHandle(orgSlug, name)}
                </code>
                .
              </>
            ) : (
              "Lowercase letters, digits and hyphens, like deployer or nightly-etl."
            )
          }
        >
          <Input
            id='api-access-account-name'
            value={name}
            onChange={(e) => setName(toSlugInput(e.target.value))}
            placeholder='deployer'
            className='font-mono text-xs'
            autoComplete='off'
            spellCheck={false}
            autoFocus
            aria-invalid={touched && !!nameError}
            data-testid='api-access-account-name'
          />
        </FormField>
      )}

      <FormField htmlFor='api-access-account-description' label='Description' aside='optional'>
        <Textarea
          id='api-access-account-description'
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          placeholder='What uses this account, and who to ask about it.'
          rows={2}
          className='text-xs'
          data-testid='api-access-account-description'
        />
      </FormField>

      <FormField label='Role' hint={ROLE_HINTS[role]}>
        <ToggleGroup
          type='single'
          variant='outline'
          size='sm'
          value={role}
          onValueChange={(v) => {
            if (v) setRole(v as ServiceAccountRole);
          }}
          aria-label='Role'
          className='justify-start'
        >
          <ToggleGroupItem
            value='member'
            className='text-xs'
            data-testid='api-access-account-role-member'
          >
            Member
          </ToggleGroupItem>
          <ToggleGroupItem
            value='admin'
            className='text-xs'
            data-testid='api-access-account-role-admin'
          >
            Admin
          </ToggleGroupItem>
        </ToggleGroup>
      </FormField>

      {serverError && (
        <p className='text-destructive text-xs' role='alert' data-testid='api-access-account-error'>
          {serverError}
        </p>
      )}

      <div className='flex justify-end gap-2'>
        <Button type='button' variant='outline' size='sm' onClick={onDone}>
          Cancel
        </Button>
        <Button
          type='submit'
          size='sm'
          disabled={isPending}
          data-testid='api-access-account-submit'
        >
          {account
            ? isPending
              ? "Saving..."
              : "Save changes"
            : isPending
              ? "Creating..."
              : "Create service account"}
        </Button>
      </div>
    </form>
  );
}
