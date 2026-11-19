import { Plus, X } from "lucide-react";
import { type FormEvent, useState } from "react";
import { toast } from "sonner";
import { CopyBlock } from "@/components/CopyBlock";
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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { useCreateIssuerTrustPolicy, useUpdateIssuerTrustPolicy } from "@/hooks/api/orgApiAccess";
import { authOrigin } from "@/libs/orgSubdomain";
import type {
  IssuerTrustPolicy,
  OidcIssuer,
  ServiceAccount,
  ServiceAccountRole
} from "@/types/orgApiAccess";
import { AccessPicker } from "../shared/AccessPicker";
import { FormField } from "../shared/FormField";
import { usePickableApps } from "../shared/usePickableLists";
import { describeApiError } from "../utils/errors";
import {
  awsRoleOf,
  buildClientSnippet,
  buildCreateTrustPolicyRequest,
  buildUpdateTrustPolicyRequest,
  conditionRow,
  emptyTrustPolicyForm,
  formFromTrustPolicy,
  presetOf,
  type TrustPolicyFormState,
  validateTrustPolicyForm
} from "../utils/issuerTrustForm";

interface TrustPolicyDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  orgId: string;
  /** The issuer a new policy trusts. */
  issuer: OidcIssuer | null;
  /** The org's service accounts: the one a policy acts as is picked from these. */
  accounts: ServiceAccount[];
  /** Set to edit that policy's grants; omit to create one. */
  policy?: IssuerTrustPolicy | null;
  onCreated: (policy: IssuerTrustPolicy) => void;
}

export function TrustPolicyDialog({
  open,
  onOpenChange,
  orgId,
  issuer,
  accounts,
  policy,
  onCreated
}: TrustPolicyDialogProps) {
  const issuerName = policy?.issuer.name ?? issuer?.name ?? "";
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className='max-h-[90vh] overflow-y-auto sm:max-w-lg'
        data-testid='api-access-trust-policy-dialog'
      >
        <DialogHeader>
          <DialogTitle className='text-base'>
            {policy ? "Edit trust policy access" : `Trust tokens from ${issuerName}`}
          </DialogTitle>
          <DialogDescription className='text-xs'>
            A token from {issuerName} whose claims meet the conditions may act as the service
            account for 15 minutes at a time. If more than one policy matches a token, it is
            refused.
          </DialogDescription>
        </DialogHeader>
        {open && (policy || issuer) && (
          <TrustPolicyForm
            orgId={orgId}
            issuer={issuer}
            accounts={accounts}
            policy={policy ?? null}
            onDone={() => onOpenChange(false)}
            onCreated={onCreated}
          />
        )}
      </DialogContent>
    </Dialog>
  );
}

function TrustPolicyForm({
  orgId,
  issuer,
  accounts,
  policy,
  onDone,
  onCreated
}: {
  orgId: string;
  issuer: OidcIssuer | null;
  accounts: ServiceAccount[];
  policy: IssuerTrustPolicy | null;
  onDone: () => void;
  onCreated: (policy: IssuerTrustPolicy) => void;
}) {
  const create = useCreateIssuerTrustPolicy();
  const update = useUpdateIssuerTrustPolicy();
  const apps = usePickableApps(orgId);
  const enabled = accounts.filter((a) => a.disabled_at === null);
  const [form, setForm] = useState<TrustPolicyFormState>(() =>
    policy
      ? formFromTrustPolicy(policy)
      : emptyTrustPolicyForm(
          issuer ? presetOf(issuer) : "custom",
          issuer?.id ?? "",
          // The only enabled account is the one it acts as, unless picked otherwise.
          enabled.length === 1 ? enabled[0].id : ""
        )
  );
  const [touched, setTouched] = useState(false);
  const [serverError, setServerError] = useState<string | null>(null);
  const mode = policy ? "edit" : "create";
  const errors = validateTrustPolicyForm(form, { mode });
  const shown = touched ? errors : {};
  const isPending = create.isPending || update.isPending;
  const set = (patch: Partial<TrustPolicyFormState>) => setForm((f) => ({ ...f, ...patch }));
  const account = accounts.find((a) => a.id === form.accountId);
  const accountRole: ServiceAccountRole = account?.org_role ?? "member";

  const setCondition = (index: number, patch: Partial<{ claim: string; value: string }>) =>
    setForm((f) => ({
      ...f,
      conditions: f.conditions.map((c, i) => (i === index ? { ...c, ...patch } : c))
    }));

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    setTouched(true);
    if (Object.keys(errors).length > 0 || isPending) return;
    setServerError(null);
    try {
      if (policy) {
        await update.mutateAsync({
          orgId,
          saId: policy.service_account.id,
          policyId: policy.id,
          request: buildUpdateTrustPolicyRequest(form, accountRole)
        });
        toast.success("Saved the policy");
      } else {
        const created = await create.mutateAsync({
          orgId,
          saId: form.accountId,
          request: buildCreateTrustPolicyRequest(form, accountRole)
        });
        onCreated(created);
      }
      onDone();
    } catch (err) {
      setServerError(describeApiError(err, "Couldn't save the policy."));
    }
  };

  const fixed = policy ? "A policy's account and conditions can't change. Add another." : undefined;

  return (
    <form onSubmit={handleSubmit} className='flex min-w-0 flex-col gap-4 pt-1' noValidate>
      <FormField
        htmlFor='api-access-trust-account'
        label='Acts as service account'
        error={shown.accountId}
        hint={fixed}
      >
        <Select
          value={form.accountId}
          onValueChange={(value) => set({ accountId: value })}
          disabled={!!policy}
        >
          <SelectTrigger
            id='api-access-trust-account'
            className='text-xs'
            data-testid='api-access-trust-account'
          >
            <SelectValue placeholder='Pick a service account' />
          </SelectTrigger>
          <SelectContent>
            {(policy ? [policy.service_account] : enabled).map((a) => (
              <SelectItem key={a.id} value={a.id} className='text-xs'>
                {a.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </FormField>

      {form.preset === "aws" ? (
        <FormField
          htmlFor='api-access-trust-role'
          label='IAM role ARN'
          error={shown.roleArn}
          hint={
            fixed ??
            "The role the workload runs as, such as an ECS task role. Tokens STS signs for that role, and no other, match."
          }
        >
          <Input
            id='api-access-trust-role'
            value={form.roleArn}
            onChange={(e) => set({ roleArn: e.target.value })}
            placeholder='arn:aws:iam::123456789012:role/ferret-task'
            className='font-mono text-xs'
            autoComplete='off'
            spellCheck={false}
            disabled={!!policy}
            aria-invalid={!!shown.roleArn}
            data-testid='api-access-trust-role'
          />
        </FormField>
      ) : (
        <FormField
          label='Claim conditions'
          error={shown.conditions}
          hint={
            fixed ??
            "Each claim must have exactly this value, or start with what comes before a trailing '*'. Every condition must hold."
          }
        >
          <div className='flex flex-col gap-2' data-testid='api-access-trust-conditions'>
            {form.conditions.map((condition, index) => (
              <div key={condition.key} className='flex items-center gap-2'>
                <Input
                  value={condition.claim}
                  onChange={(e) => setCondition(index, { claim: e.target.value })}
                  placeholder='sub'
                  aria-label={`Claim ${index + 1}`}
                  className='w-1/3 font-mono text-xs'
                  autoComplete='off'
                  spellCheck={false}
                  disabled={!!policy}
                  data-testid='api-access-trust-condition-claim'
                />
                <Input
                  value={condition.value}
                  onChange={(e) => setCondition(index, { value: e.target.value })}
                  placeholder='project_path:acme/app:ref_type:branch:ref:main'
                  aria-label={`Value ${index + 1}`}
                  className='flex-1 font-mono text-xs'
                  autoComplete='off'
                  spellCheck={false}
                  disabled={!!policy}
                  data-testid='api-access-trust-condition-value'
                />
                {!policy && form.conditions.length > 1 && (
                  <Button
                    type='button'
                    variant='ghost'
                    size='icon'
                    className='size-7 text-muted-foreground'
                    aria-label={`Remove condition ${index + 1}`}
                    onClick={() =>
                      set({ conditions: form.conditions.filter((_, i) => i !== index) })
                    }
                  >
                    <X className='size-4' aria-hidden />
                  </Button>
                )}
              </div>
            ))}
            {!policy && (
              <Button
                type='button'
                variant='outline'
                size='sm'
                className='self-start'
                onClick={() => set({ conditions: [...form.conditions, conditionRow()] })}
                data-testid='api-access-trust-condition-add'
              >
                <Plus className='size-4' aria-hidden />
                Add condition
              </Button>
            )}
          </div>
        </FormField>
      )}

      <FormField label='What it may do' error={shown.access}>
        <AccessPicker
          value={form.access}
          onChange={(access) => set({ access })}
          accountRole={accountRole}
          // No single workspace: the whole organization, or apps only.
          apps={apps}
          testId='api-access-trust-access'
        />
      </FormField>

      {serverError && (
        <p className='text-destructive text-xs' role='alert' data-testid='api-access-trust-error'>
          {serverError}
        </p>
      )}

      <div className='flex justify-end gap-2'>
        <Button type='button' variant='outline' size='sm' onClick={onDone}>
          Cancel
        </Button>
        <Button type='submit' size='sm' disabled={isPending} data-testid='api-access-trust-submit'>
          {policy
            ? isPending
              ? "Saving..."
              : "Save changes"
            : isPending
              ? "Adding..."
              : "Add policy"}
        </Button>
      </div>
    </form>
  );
}

interface ClientSetupDialogProps {
  policy: IssuerTrustPolicy | null;
  justCreated: boolean;
  orgSlug: string;
  onClose: () => void;
}

/**
 * The other half of a policy the AWS preset wrote: what the workload runs.
 * Shown straight after creating one, because a policy alone does nothing.
 */
export function ClientSetupDialog({
  policy,
  justCreated,
  orgSlug,
  onClose
}: ClientSetupDialogProps) {
  const role = policy ? awsRoleOf(policy.conditions) : null;
  return (
    <Dialog open={policy !== null && role !== null} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className='sm:max-w-xl' data-testid='api-access-client-setup-dialog'>
        <DialogHeader>
          <DialogTitle className='text-base'>
            {justCreated
              ? "Policy saved. Now set up the workload"
              : "Workload setup for this policy"}
          </DialogTitle>
          <DialogDescription className='text-xs'>
            {policy && role && (
              <>
                Run this where <code className='font-mono'>{role}</code> is the workload's role. STS
                signs a five-minute token for this deployment; the exchange trades it for an Oxygen
                token that lasts 15 minutes. Ask STS for a new one each time: each token is
                exchanged once.
              </>
            )}
          </DialogDescription>
        </DialogHeader>
        {policy && role && (
          // min-w-0 lets the unwrapped block scroll inside the dialog instead of widening it.
          <div className='flex min-w-0 flex-col'>
            <CopyBlock
              text={buildClientSnippet({
                accountId: policy.service_account.id,
                orgSlug,
                accountName: policy.service_account.name,
                // The exchange takes only the audience of the deployment's own address, so
                // never an org subdomain's: authOrigin() is that address there.
                origin: authOrigin()
              })}
              label='Copy commands'
              testId='api-access-client-setup'
              nowrap
            />
          </div>
        )}
        <DialogFooter>
          <Button size='sm' onClick={onClose} data-testid='api-access-client-setup-done'>
            Done
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
