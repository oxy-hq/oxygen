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
import { Switch } from "@/components/ui/shadcn/switch";
import { useCreateTrustPolicy, useUpdateTrustPolicy } from "@/hooks/api/orgApiAccess";
import type { ServiceAccount, TrustPolicy } from "@/types/orgApiAccess";
import { AccessPicker } from "../../../shared/AccessPicker";
import { FormField } from "../../../shared/FormField";
import { usePickableApps, usePickableWorkspaces } from "../../../shared/usePickableLists";
import { describeApiError, isApiErrorCode } from "../../../utils/errors";
import {
  buildCreateTrustPolicyRequest,
  buildUpdateTrustPolicyRequest,
  EMPTY_TRUST_POLICY_FORM,
  environmentRequiredFor,
  formFromPolicy,
  hasErrors,
  normalizeRepository,
  refPatternHint,
  type TrustPolicyFormState,
  validateTrustPolicyForm
} from "../../../utils/trustPolicyForm";
import { EnvironmentField, RepositoryIdFields } from "./TrustPolicyFields";

interface TrustPolicyDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  orgId: string;
  account: ServiceAccount;
  /** Set to edit that policy; omit to create one. */
  policy?: TrustPolicy | null;
  /** The org's policy, when it is known. Unknown leaves the decision to the server. */
  requireEnvironment: boolean;
  onCreated: (policy: TrustPolicy) => void;
}

export function TrustPolicyDialog({
  open,
  onOpenChange,
  orgId,
  account,
  policy,
  requireEnvironment,
  onCreated
}: TrustPolicyDialogProps) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className='max-h-[90vh] overflow-y-auto sm:max-w-lg'
        data-testid='api-access-policy-dialog'
      >
        <DialogHeader>
          <DialogTitle className='text-base'>
            {policy ? "Edit trusted-access policy" : "Trust a GitHub Actions workflow"}
          </DialogTitle>
          <DialogDescription className='text-xs'>
            A run that matches every rule below may act as {account.name} for 15 minutes. Nothing is
            stored in the repository.
          </DialogDescription>
        </DialogHeader>
        {open && (
          <TrustPolicyForm
            orgId={orgId}
            account={account}
            policy={policy ?? null}
            requireEnvironment={requireEnvironment}
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
  account,
  policy,
  requireEnvironment,
  onDone,
  onCreated
}: {
  orgId: string;
  account: ServiceAccount;
  policy: TrustPolicy | null;
  requireEnvironment: boolean;
  onDone: () => void;
  onCreated: (policy: TrustPolicy) => void;
}) {
  const create = useCreateTrustPolicy();
  const update = useUpdateTrustPolicy();
  const workspaces = usePickableWorkspaces(orgId);
  const apps = usePickableApps(orgId);
  const [form, setForm] = useState<TrustPolicyFormState>(() =>
    policy ? formFromPolicy(policy) : EMPTY_TRUST_POLICY_FORM
  );
  const [touched, setTouched] = useState(false);
  // Both are learned from the server: it couldn't resolve the repository's ids,
  // or it requires an environment and the org's policy hadn't loaded here.
  const [needsIds, setNeedsIds] = useState(false);
  const [serverRequiresEnvironment, setServerRequiresEnvironment] = useState(false);
  const [serverError, setServerError] = useState<string | null>(null);

  const mode = policy ? "edit" : "create";
  const mustHaveEnvironment =
    environmentRequiredFor(policy, requireEnvironment) || serverRequiresEnvironment;
  const errors = validateTrustPolicyForm(form, {
    mode,
    requireEnvironment: mustHaveEnvironment,
    needsIds
  });
  const shown = touched ? errors : {};
  const isPending = create.isPending || update.isPending;
  const set = (patch: Partial<TrustPolicyFormState>) => setForm((f) => ({ ...f, ...patch }));

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    setTouched(true);
    if (hasErrors(errors) || isPending) return;
    setServerError(null);
    try {
      if (policy) {
        await update.mutateAsync({
          orgId,
          saId: account.id,
          policyId: policy.id,
          request: buildUpdateTrustPolicyRequest(form, account.org_role, policy)
        });
        toast.success("Saved the policy");
      } else {
        const created = await create.mutateAsync({
          orgId,
          saId: account.id,
          request: buildCreateTrustPolicyRequest(form, account.org_role, { includeIds: needsIds })
        });
        onCreated(created);
      }
      onDone();
    } catch (err) {
      // Two refusals reshape the form instead of just reporting. The first
      // reveals the id fields, untouched, so they read as a next step and not
      // as two new mistakes; the second turns the environment from advised to
      // required, and the field then carries the contract's message itself.
      if (isApiErrorCode(err, "repository_unresolved") && !needsIds) {
        setNeedsIds(true);
        setTouched(false);
        return;
      }
      if (isApiErrorCode(err, "environment_required") && !form.environment.trim()) {
        setServerRequiresEnvironment(true);
        return;
      }
      setServerError(describeApiError(err, "Couldn't save the policy."));
    }
  };

  return (
    <form onSubmit={handleSubmit} className='flex min-w-0 flex-col gap-4 pt-1' noValidate>
      <FormField
        htmlFor='api-access-policy-repository'
        label='Repository'
        error={shown.repository}
        hint={
          policy ? "A policy's repository can't change. Add another policy instead." : undefined
        }
      >
        <Input
          id='api-access-policy-repository'
          value={form.repository}
          onChange={(e) => set({ repository: e.target.value })}
          onBlur={() => set({ repository: normalizeRepository(form.repository) })}
          placeholder='acme/storefront'
          className='font-mono text-xs'
          autoComplete='off'
          spellCheck={false}
          autoFocus={!policy}
          disabled={!!policy}
          aria-invalid={!!shown.repository}
          data-testid='api-access-policy-repository'
        />
      </FormField>

      {needsIds && !policy && <RepositoryIdFields form={form} set={set} errors={shown} />}

      <FormField
        htmlFor='api-access-policy-workflow'
        label='Workflow file'
        error={shown.workflowPath}
        hint='The file under .github/workflows that runs. If it calls a reusable workflow, name the reusable one.'
      >
        <Input
          id='api-access-policy-workflow'
          value={form.workflowPath}
          onChange={(e) => set({ workflowPath: e.target.value })}
          placeholder='release.yml'
          className='font-mono text-xs'
          autoComplete='off'
          spellCheck={false}
          aria-invalid={!!shown.workflowPath}
          data-testid='api-access-policy-workflow'
        />
      </FormField>

      <EnvironmentField
        value={form.environment}
        onChange={(environment) => set({ environment })}
        required={mustHaveEnvironment}
        error={shown.environment}
      />

      <FormField
        htmlFor='api-access-policy-ref'
        label='Ref pattern'
        aside='optional'
        hint={
          refPatternHint(form.refPattern) ??
          "Limit which branch or tag may run it, like refs/heads/main or refs/tags/v*."
        }
      >
        <Input
          id='api-access-policy-ref'
          value={form.refPattern}
          onChange={(e) => set({ refPattern: e.target.value })}
          placeholder='refs/heads/main'
          className='font-mono text-xs'
          autoComplete='off'
          spellCheck={false}
          data-testid='api-access-policy-ref'
        />
      </FormField>

      <FormField label='What it may do' error={shown.access}>
        <AccessPicker
          value={form.access}
          onChange={(access) => set({ access })}
          accountRole={account.org_role}
          workspaces={workspaces}
          apps={apps}
          testId='api-access-policy-access'
        />
      </FormField>

      <div className='flex items-start justify-between gap-4 rounded-md border p-3 text-xs'>
        <div className='flex flex-col gap-0.5'>
          <label htmlFor='api-access-policy-self-hosted' className='font-medium'>
            Allow self-hosted runners
          </label>
          <p
            id='api-access-policy-self-hosted-hint'
            className='text-muted-foreground leading-relaxed'
          >
            Off by default: only GitHub's own runners match. A self-hosted runner is a machine you
            operate, so turn this on only if you trust everything that runs on it.
          </p>
        </div>
        <Switch
          id='api-access-policy-self-hosted'
          checked={form.allowSelfHosted}
          onCheckedChange={(allowSelfHosted) => set({ allowSelfHosted })}
          aria-describedby='api-access-policy-self-hosted-hint'
          data-testid='api-access-policy-self-hosted'
        />
      </div>

      {serverError && (
        <p className='text-destructive text-xs' role='alert' data-testid='api-access-policy-error'>
          {serverError}
        </p>
      )}

      <div className='flex justify-end gap-2'>
        <Button type='button' variant='outline' size='sm' onClick={onDone}>
          Cancel
        </Button>
        <Button type='submit' size='sm' disabled={isPending} data-testid='api-access-policy-submit'>
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
