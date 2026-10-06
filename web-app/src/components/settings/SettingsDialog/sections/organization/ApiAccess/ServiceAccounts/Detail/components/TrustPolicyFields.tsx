import { TriangleAlert } from "lucide-react";
import { Input } from "@/components/ui/shadcn/input";
import { FormField } from "../../../shared/FormField";
import {
  ENVIRONMENT_UNPROTECTED_WARNING,
  environmentState,
  normalizeRepository,
  type TrustPolicyFormState
} from "../../../utils/trustPolicyForm";

/**
 * The environment is the one field that decides who can use the policy: with
 * it, GitHub's protection rules gate each run; without it, a push is enough.
 * So an empty field never passes silently — it is either refused (the org
 * requires one) or carries the warning.
 */
export function EnvironmentField({
  value,
  onChange,
  required,
  error
}: {
  value: string;
  onChange: (value: string) => void;
  required: boolean;
  error: string | undefined;
}) {
  const state = environmentState(value, required);
  return (
    <FormField
      htmlFor='api-access-policy-environment'
      label='Environment'
      aside={required ? "required by this organization" : "strongly recommended"}
      error={error}
      hint='The GitHub environment the job runs in, like production. Its protection rules decide who can run the workflow.'
    >
      <Input
        id='api-access-policy-environment'
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder='production'
        className='font-mono text-xs'
        autoComplete='off'
        spellCheck={false}
        aria-invalid={!!error}
        data-testid='api-access-policy-environment'
      />
      {state === "unprotected" && (
        <div
          className='flex gap-2 rounded-md border border-warning/40 bg-warning/10 p-2.5 text-xs'
          role='status'
          data-testid='api-access-policy-environment-warning'
        >
          <TriangleAlert className='mt-0.5 size-3.5 shrink-0 text-warning' aria-hidden />
          <p className='leading-relaxed'>{ENVIRONMENT_UNPROTECTED_WARNING}</p>
        </div>
      )}
    </FormField>
  );
}

/** Shown only after the server couldn't look the repository up itself. */
export function RepositoryIdFields({
  form,
  set,
  errors
}: {
  form: TrustPolicyFormState;
  set: (patch: Partial<TrustPolicyFormState>) => void;
  errors: { repositoryId?: string; repositoryOwnerId?: string };
}) {
  const repository = normalizeRepository(form.repository) || "OWNER/REPO";
  return (
    <div
      className='flex flex-col gap-3 rounded-md border bg-muted/40 p-3'
      data-testid='api-access-policy-ids'
    >
      <p className='text-xs leading-relaxed'>
        We couldn't look up this repository's ids, which happens for a private repository this
        organization hasn't connected. Enter them, so the policy keeps matching this exact
        repository even if it is renamed or its name is taken over.
      </p>
      <div className='grid gap-3 sm:grid-cols-2'>
        <FormField
          htmlFor='api-access-policy-repo-id'
          label='Repository id'
          error={errors.repositoryId}
        >
          <Input
            id='api-access-policy-repo-id'
            value={form.repositoryId}
            onChange={(e) => set({ repositoryId: e.target.value })}
            inputMode='numeric'
            placeholder='123456789'
            className='font-mono text-xs'
            autoComplete='off'
            aria-invalid={!!errors.repositoryId}
            data-testid='api-access-policy-repo-id'
          />
        </FormField>
        <FormField
          htmlFor='api-access-policy-owner-id'
          label='Owner id'
          error={errors.repositoryOwnerId}
        >
          <Input
            id='api-access-policy-owner-id'
            value={form.repositoryOwnerId}
            onChange={(e) => set({ repositoryOwnerId: e.target.value })}
            inputMode='numeric'
            placeholder='987654'
            className='font-mono text-xs'
            autoComplete='off'
            aria-invalid={!!errors.repositoryOwnerId}
            data-testid='api-access-policy-owner-id'
          />
        </FormField>
      </div>
      <p className='text-muted-foreground text-xs leading-relaxed'>
        Both come from one command, repository id first:{" "}
        <code className='break-all font-mono text-foreground'>
          gh api repos/{repository} --jq '.id, .owner.id'
        </code>
      </p>
    </div>
  );
}
