import { Bot, Plus } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { useServiceAccounts } from "@/hooks/api/orgApiAccess";
import type { Organization } from "@/types/organization";
import { EmptyState, ListState } from "../shared/ListState";
import { ServiceAccountDialog } from "./components/ServiceAccountDialog";
import { ServiceAccountTable } from "./components/ServiceAccountTable";
import { ServiceAccountDetail } from "./Detail";

interface ServiceAccountsProps {
  org: Organization;
  /** The account whose page is open, or `null` for the list. Owned by the section. */
  selectedId: string | null;
  onSelect: (accountId: string | null) => void;
}

/**
 * The org's service accounts: the list, or one account's page with its tokens
 * and trusted-access policies.
 *
 * The page replaces the list in place rather than opening over it. An account
 * holds two lists that each open their own dialogs, and a dialog stacked on a
 * dialog stacked on Settings is one layer more than anyone can keep track of.
 */
export function ServiceAccounts({ org, selectedId, onSelect }: ServiceAccountsProps) {
  const { data, isPending, error, refetch } = useServiceAccounts(org.id);
  const [creating, setCreating] = useState(false);
  const accounts = data ?? [];
  const selected = selectedId ? accounts.find((a) => a.id === selectedId) : undefined;

  if (selected) {
    return (
      <ServiceAccountDetail
        org={org}
        account={selected}
        takenNames={accounts.map((a) => a.name)}
        onBack={() => onSelect(null)}
      />
    );
  }

  const createButton = (
    <Button size='sm' onClick={() => setCreating(true)} data-testid='api-access-account-create'>
      <Plus className='size-4' aria-hidden />
      New service account
    </Button>
  );

  return (
    <div className='flex flex-col gap-4' data-testid='api-access-accounts'>
      <div className='flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between'>
        <p className='max-w-xl text-muted-foreground text-xs leading-relaxed'>
          An identity for automation, owned by the organization instead of a person. Its access
          doesn't leave when someone does. Give it a token, or let a GitHub Actions workflow act as
          it with no stored secret.
        </p>
        {accounts.length > 0 && <div className='shrink-0'>{createButton}</div>}
      </div>

      <ListState
        what='service accounts'
        isPending={isPending}
        // A failed background refetch keeps showing what was already loaded.
        error={data ? null : error}
        isEmpty={accounts.length === 0}
        onRetry={refetch}
        testId='api-access-accounts'
        empty={
          <EmptyState
            icon={Bot}
            title='No service accounts yet'
            action={createButton}
            testId='api-access-accounts-empty'
          >
            Create one for each thing that calls Oxygen on its own: a deploy pipeline, a nightly
            job, an integration. Then each can be given only the access it needs, and switched off
            without touching the others.
          </EmptyState>
        }
      >
        <ServiceAccountTable
          orgId={org.id}
          orgSlug={org.slug}
          accounts={accounts}
          onOpen={(account) => onSelect(account.id)}
        />
      </ListState>

      <ServiceAccountDialog
        open={creating}
        onOpenChange={setCreating}
        orgId={org.id}
        orgSlug={org.slug}
        takenNames={accounts.map((a) => a.name)}
        // Land on the new account's page: the next step is always a token or a policy.
        onCreated={(account) => onSelect(account.id)}
      />
    </div>
  );
}
