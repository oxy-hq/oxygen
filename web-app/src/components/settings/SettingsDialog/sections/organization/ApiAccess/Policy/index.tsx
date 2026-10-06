import { type FormEvent, type ReactNode, useId, useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import { Switch } from "@/components/ui/shadcn/switch";
import { useTokenPolicy, useUpdateTokenPolicy } from "@/hooks/api/orgApiAccess";
import type { TokenPolicy } from "@/types/orgApiAccess";
import type { Organization } from "@/types/organization";
import { ListState } from "../shared/ListState";
import { describeApiError } from "../utils/errors";
import {
  formToPolicy,
  isPolicyDirty,
  maxLifetimeError,
  type PolicyFormState,
  policyChangeNotes,
  policyToForm
} from "../utils/policyForm";

/**
 * The org's rules for tokens. Three settings, and one idea that matters more
 * than any of them: a rule here blocks a token *for this organization*, it
 * never revokes it. That is stated first, before anything can be switched.
 */
export function Policy({ org }: { org: Organization }) {
  const { data, isPending, error, refetch } = useTokenPolicy(org.id);

  return (
    <div className='flex flex-col gap-5' data-testid='api-access-policy'>
      <div className='flex max-w-xl flex-col gap-2 text-xs leading-relaxed'>
        <p>
          A token that breaks a rule here is{" "}
          <strong className='font-semibold'>blocked for {org.name}, not revoked</strong>. It stops
          working in this organization and keeps working everywhere else, and it works here again as
          soon as it complies. Its owner sees the reason in their own token list.
        </p>
        <p className='text-muted-foreground'>
          Legacy API keys are exempt from every rule on this page. Only a legacy API key's owner can
          extend or revoke it.
        </p>
      </div>

      <ListState
        what='policy settings'
        isPending={isPending}
        // A failed background refetch keeps showing what was already loaded.
        error={data ? null : error}
        isEmpty={false}
        onRetry={refetch}
        testId='api-access-policy'
        empty={null}
      >
        {/* Keyed on the saved policy, so a save (or a refetch) resets the form to it. */}
        {data && <PolicyForm key={JSON.stringify(data)} orgId={org.id} saved={data} />}
      </ListState>
    </div>
  );
}

function PolicyForm({ orgId, saved }: { orgId: string; saved: TokenPolicy }) {
  const update = useUpdateTokenPolicy();
  const [form, setForm] = useState<PolicyFormState>(() => policyToForm(saved));
  const [serverError, setServerError] = useState<string | null>(null);
  const daysId = useId();

  const lifetimeError = maxLifetimeError(form);
  const dirty = isPolicyDirty(form, saved);
  const notes = policyChangeNotes(form, saved);
  const set = (patch: Partial<PolicyFormState>) => setForm((f) => ({ ...f, ...patch }));

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    const policy = formToPolicy(form);
    if (!policy || update.isPending) return;
    setServerError(null);
    try {
      await update.mutateAsync({ orgId, policy });
      toast.success("Saved the policy");
    } catch (err) {
      setServerError(describeApiError(err, "Couldn't save the policy."));
    }
  };

  return (
    <form onSubmit={handleSubmit} className='flex flex-col gap-4' noValidate>
      <div className='divide-y rounded-lg border'>
        <Setting
          title='Limit how long a token can last'
          description='New tokens can’t be created with a longer life, and existing ones can’t be extended past it. Off means no limit, so a token may never expire.'
          checked={form.limitLifetime}
          onCheckedChange={(limitLifetime) => set({ limitLifetime })}
          testId='api-access-policy-limit-lifetime'
        >
          {form.limitLifetime && (
            <div className='flex flex-col gap-1.5 pt-1'>
              <div className='flex items-center gap-2'>
                <Input
                  id={daysId}
                  value={form.maxLifetimeDays}
                  onChange={(e) => set({ maxLifetimeDays: e.target.value })}
                  inputMode='numeric'
                  className='h-8 w-24 text-xs tabular-nums'
                  aria-label='Maximum lifetime in days'
                  aria-invalid={!!lifetimeError}
                  data-testid='api-access-policy-max-days'
                />
                <label htmlFor={daysId} className='text-xs'>
                  days at most
                </label>
              </div>
              {lifetimeError && (
                <p className='text-destructive text-xs' role='alert'>
                  {lifetimeError}
                </p>
              )}
            </div>
          )}
        </Setting>

        <Setting
          title='Allow all-access personal tokens'
          description='An all-access token reaches everything its owner can, in every organization they belong to. Turn this off and only tokens that name this organization explicitly work here.'
          checked={form.allowAllAccessTokens}
          onCheckedChange={(allowAllAccessTokens) => set({ allowAllAccessTokens })}
          testId='api-access-policy-allow-all-access'
        />

        <Setting
          title='Require an environment on trusted-access policies'
          description='A GitHub environment puts its protection rules, like required reviewers, in front of every run. Without one, anyone who can push to a trusted repository could get a token.'
          checked={form.requireEnvironment}
          onCheckedChange={(requireEnvironment) => set({ requireEnvironment })}
          testId='api-access-policy-require-environment'
        />
      </div>

      {notes.length > 0 && (
        <div
          className='flex flex-col gap-1.5 rounded-md border border-warning/40 bg-warning/10 p-3 text-xs'
          role='status'
          data-testid='api-access-policy-notes'
        >
          <p className='font-medium'>What saving changes right away</p>
          <ul className='flex list-disc flex-col gap-1 pl-4 leading-relaxed'>
            {notes.map((note) => (
              <li key={note}>{note}</li>
            ))}
          </ul>
        </div>
      )}

      {serverError && (
        <p className='text-destructive text-xs' role='alert' data-testid='api-access-policy-error'>
          {serverError}
        </p>
      )}

      <div className='flex justify-end gap-2'>
        <Button
          type='button'
          variant='ghost'
          size='sm'
          onClick={() => setForm(policyToForm(saved))}
          disabled={!dirty || update.isPending}
          data-testid='api-access-policy-discard'
        >
          Discard changes
        </Button>
        <Button
          type='submit'
          size='sm'
          disabled={!dirty || !!lifetimeError || update.isPending}
          data-testid='api-access-policy-save'
        >
          {update.isPending ? "Saving..." : "Save policy"}
        </Button>
      </div>
    </form>
  );
}

/** One rule: what it is, what it does, and its switch. Extra controls sit under the text. */
function Setting({
  title,
  description,
  checked,
  onCheckedChange,
  testId,
  children
}: {
  title: string;
  description: string;
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
  testId: string;
  children?: ReactNode;
}) {
  const titleId = useId();
  const descriptionId = useId();
  return (
    <div className='flex items-start justify-between gap-4 p-4'>
      <div className='flex min-w-0 flex-col gap-1'>
        <p id={titleId} className='font-medium text-sm'>
          {title}
        </p>
        <p id={descriptionId} className='max-w-lg text-muted-foreground text-xs leading-relaxed'>
          {description}
        </p>
        {children}
      </div>
      <Switch
        checked={checked}
        onCheckedChange={onCheckedChange}
        aria-labelledby={titleId}
        aria-describedby={descriptionId}
        data-testid={testId}
      />
    </div>
  );
}
