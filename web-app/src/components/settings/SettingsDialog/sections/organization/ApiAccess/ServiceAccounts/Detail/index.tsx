import { ArrowLeft, Info } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import type { ServiceAccount } from "@/types/orgApiAccess";
import type { Organization } from "@/types/organization";
import { CopyButton } from "../../shared/CopyBlock";
import { serviceAccountHandle } from "../../utils/slug";
import { formatDay } from "../../utils/tokens";
import { ServiceAccountActions } from "../components/ServiceAccountActions";
import { RoleLabel, ServiceAccountStatusBadge } from "../components/ServiceAccountStatusBadge";
import { TokensPane } from "./components/TokensPane";
import { TrustPoliciesPane } from "./components/TrustPoliciesPane";

interface ServiceAccountDetailProps {
  org: Organization;
  account: ServiceAccount;
  takenNames: string[];
  onBack: () => void;
}

/**
 * One service account: who it is, then the two ways something can act as it.
 *
 * Tokens and trusted access are stacked, not tabbed, the way Crew stacks
 * workers over kiosks: they are alternatives an admin weighs against each
 * other, and seeing a long-lived token beside the policy that could replace it
 * is the point.
 */
export function ServiceAccountDetail({
  org,
  account,
  takenNames,
  onBack
}: ServiceAccountDetailProps) {
  const handle = serviceAccountHandle(org.slug, account.name);
  const disabled = account.disabled_at !== null;

  return (
    <div className='flex flex-col gap-6' data-testid='api-access-account-detail'>
      <div className='flex flex-col gap-3'>
        <Button
          variant='ghost'
          size='sm'
          className='-ml-2 h-7 self-start px-2 text-muted-foreground text-xs'
          onClick={onBack}
          data-testid='api-access-account-back'
        >
          <ArrowLeft className='size-3.5' aria-hidden />
          Service accounts
        </Button>

        <div className='flex items-start justify-between gap-3'>
          <div className='flex min-w-0 flex-col gap-1.5'>
            <div className='flex flex-wrap items-center gap-2'>
              <h4 className='break-all font-mono font-semibold text-base'>{account.name}</h4>
              <ServiceAccountStatusBadge account={account} />
            </div>
            {account.description && (
              <p className='max-w-xl text-muted-foreground text-xs leading-relaxed'>
                {account.description}
              </p>
            )}
          </div>
          <ServiceAccountActions
            orgId={org.id}
            orgSlug={org.slug}
            account={account}
            takenNames={takenNames}
            onDeleted={onBack}
          />
        </div>

        <dl className='grid grid-cols-[auto_1fr] items-center gap-x-6 gap-y-1.5 text-xs'>
          <dt className='text-muted-foreground'>Handle</dt>
          <dd className='flex min-w-0 items-center gap-2'>
            <code
              className='truncate rounded-sm bg-muted px-1.5 py-0.5 font-mono'
              data-testid='api-access-account-handle'
            >
              {handle}
            </code>
            <CopyButton text={handle} label='Copy handle' testId='api-access-account-handle-copy' />
          </dd>
          <dt className='text-muted-foreground'>ID</dt>
          <dd className='flex min-w-0 flex-wrap items-center gap-2'>
            <code
              className='truncate rounded-sm bg-muted px-1.5 py-0.5 font-mono'
              data-testid='api-access-account-id'
            >
              {account.id}
            </code>
            <CopyButton text={account.id} label='Copy ID' testId='api-access-account-id-copy' />
            <span className='text-muted-foreground'>A workflow names the account by this ID.</span>
          </dd>
          <dt className='text-muted-foreground'>Role</dt>
          <dd>
            <RoleLabel role={account.org_role} />
          </dd>
          <dt className='text-muted-foreground'>Created</dt>
          <dd>
            {formatDay(account.created_at)}
            {account.created_by && ` by ${account.created_by.label}`}
          </dd>
        </dl>

        {disabled && (
          <div
            className='flex gap-2 rounded-md border bg-muted/40 p-3 text-xs'
            data-testid='api-access-account-disabled-notice'
          >
            <Info className='mt-0.5 size-3.5 shrink-0 text-muted-foreground' aria-hidden />
            <p className='leading-relaxed'>
              This account is disabled, so its tokens and trusted-access policies don't work. Enable
              it from the menu above to bring them back as they were.
            </p>
          </div>
        )}
      </div>

      <TokensPane org={org} account={account} />
      <TrustPoliciesPane org={org} account={account} />
    </div>
  );
}
