import { Plus, ShieldCheck } from "lucide-react";
import { useState } from "react";
import TableWrapper from "@/components/settings/components/TableWrapper";
import { Button } from "@/components/ui/shadcn/button";
import { Table, TableBody, TableHead, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import { useTokenPolicy, useTrustPolicies } from "@/hooks/api/orgApiAccess";
import type { ServiceAccount, TrustPolicy } from "@/types/orgApiAccess";
import type { Organization } from "@/types/organization";
import { EmptyState, ListState } from "../../../shared/ListState";
import { DEFAULT_TOKEN_POLICY } from "../../../utils/policyForm";
import { PaneHeader } from "./PaneHeader";
import { TrustPolicyDialog } from "./TrustPolicyDialog";
import { TrustPolicyRow } from "./TrustPolicyRow";
import { WorkflowSnippetDialog } from "./WorkflowSnippetDialog";

interface TrustPoliciesPaneProps {
  org: Organization;
  account: ServiceAccount;
}

/**
 * Trusted access: the GitHub Actions workflows allowed to act as this account
 * without a stored secret. Each policy names one workflow in one repository.
 */
export function TrustPoliciesPane({ org, account }: TrustPoliciesPaneProps) {
  const { data, isPending, error, refetch } = useTrustPolicies(org.id, account.id);
  // The loaded policy decides. The server answers its defaults for an org that
  // never saved one, so unknown (still loading) reads as that default, which is
  // "required"; the server refuses anyway if it is.
  const { data: orgPolicy } = useTokenPolicy(org.id);
  const requireEnvironment =
    orgPolicy?.require_environment_on_trust_policies ??
    DEFAULT_TOKEN_POLICY.require_environment_on_trust_policies;

  const [dialog, setDialog] = useState<{ policy: TrustPolicy | null } | null>(null);
  const [snippet, setSnippet] = useState<{ policy: TrustPolicy; justCreated: boolean } | null>(
    null
  );
  const policies = data ?? [];
  const disabled = account.disabled_at !== null;

  const addButton = (
    <Button
      size='sm'
      variant='outline'
      onClick={() => setDialog({ policy: null })}
      disabled={disabled}
      title={disabled ? "Enable the account to add a policy" : undefined}
      data-testid='api-access-policy-create'
    >
      <Plus className='size-4' aria-hidden />
      Add policy
    </Button>
  );

  return (
    <section className='flex flex-col gap-3' data-testid='api-access-policies'>
      <PaneHeader
        title='Trusted access'
        description='Let a GitHub Actions workflow act as this account with no secret to store or rotate. Each run gets its own token, good for 15 minutes.'
        action={policies.length > 0 ? addButton : undefined}
      />

      <ListState
        what='trusted-access policies'
        isPending={isPending}
        // A failed background refetch keeps showing what was already loaded.
        error={data ? null : error}
        isEmpty={policies.length === 0}
        onRetry={refetch}
        testId='api-access-policies'
        empty={
          <EmptyState
            icon={ShieldCheck}
            title='No trusted workflows'
            action={addButton}
            testId='api-access-policies-empty'
          >
            Name a repository, a workflow file and an environment, and that workflow can deploy as
            this account. There is no token to leak, and you get the workflow to paste once the
            policy is saved.
          </EmptyState>
        }
      >
        <TableWrapper>
          <Table className='text-xs' data-testid='api-access-policy-table'>
            <TableHeader>
              <TableRow>
                <TableHead className='px-3'>Repository and workflow</TableHead>
                <TableHead className='px-3'>Environment</TableHead>
                <TableHead className='px-3'>Ref</TableHead>
                <TableHead className='px-3'>Access</TableHead>
                <TableHead className='px-3'>Last used</TableHead>
                <TableHead className='px-3'>Status</TableHead>
                <TableHead className='w-10 px-3'>
                  <span className='sr-only'>Actions</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {policies.map((policy) => (
                <TrustPolicyRow
                  key={policy.id}
                  orgId={org.id}
                  saId={account.id}
                  policy={policy}
                  onEdit={(p) => setDialog({ policy: p })}
                  onShowWorkflow={(p) => setSnippet({ policy: p, justCreated: false })}
                />
              ))}
            </TableBody>
          </Table>
        </TableWrapper>
      </ListState>

      <TrustPolicyDialog
        open={dialog !== null}
        onOpenChange={(open) => !open && setDialog(null)}
        orgId={org.id}
        account={account}
        policy={dialog?.policy}
        requireEnvironment={requireEnvironment}
        // A policy alone does nothing: hand over the workflow that matches it.
        onCreated={(policy) => setSnippet({ policy, justCreated: true })}
      />
      <WorkflowSnippetDialog
        policy={snippet?.policy ?? null}
        justCreated={snippet?.justCreated ?? false}
        orgSlug={org.slug}
        accountName={account.name}
        onClose={() => setSnippet(null)}
      />
    </section>
  );
}
