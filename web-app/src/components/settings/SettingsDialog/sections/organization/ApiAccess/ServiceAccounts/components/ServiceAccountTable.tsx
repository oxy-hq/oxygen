import TableWrapper from "@/components/settings/components/TableWrapper";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from "@/components/ui/shadcn/table";
import { cn } from "@/libs/shadcn/utils";
import type { ServiceAccount } from "@/types/orgApiAccess";
import { countLabel } from "../../utils/serviceAccounts";
import { ServiceAccountActions } from "./ServiceAccountActions";
import { RoleLabel, ServiceAccountStatusBadge } from "./ServiceAccountStatusBadge";

interface ServiceAccountTableProps {
  orgId: string;
  orgSlug: string;
  accounts: ServiceAccount[];
  onOpen: (account: ServiceAccount) => void;
}

const CELL = "px-3 py-2.5 align-top max-md:px-0 max-md:py-0";

export function ServiceAccountTable({
  orgId,
  orgSlug,
  accounts,
  onOpen
}: ServiceAccountTableProps) {
  const takenNames = accounts.map((a) => a.name);
  return (
    <TableWrapper>
      <Table className='text-xs' data-testid='api-access-account-table'>
        <TableHeader>
          <TableRow>
            <TableHead className='px-3'>Name</TableHead>
            <TableHead className='px-3'>Role</TableHead>
            <TableHead className='px-3'>Tokens</TableHead>
            <TableHead className='px-3'>Trusted access</TableHead>
            <TableHead className='px-3'>Created by</TableHead>
            <TableHead className='px-3'>Status</TableHead>
            <TableHead className='w-10 px-3'>
              <span className='sr-only'>Actions</span>
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {accounts.map((account) => (
            <TableRow
              key={account.id}
              className={cn(account.disabled_at && "text-muted-foreground")}
              data-testid='api-access-account-row'
              data-account-name={account.name}
            >
              <TableCell data-label='Name' className={cn(CELL, "whitespace-normal")}>
                <button
                  type='button'
                  onClick={() => onOpen(account)}
                  className='rounded-sm text-left font-medium font-mono text-foreground underline-offset-2 hover:underline focus-visible:outline-2 focus-visible:outline-ring'
                  data-testid='api-access-account-open'
                >
                  {account.name}
                </button>
                {account.description && (
                  <p className='mt-0.5 max-w-xs text-muted-foreground'>{account.description}</p>
                )}
              </TableCell>
              <TableCell data-label='Role' className={CELL}>
                <RoleLabel role={account.org_role} />
              </TableCell>
              <TableCell data-label='Tokens' className={cn(CELL, "tabular-nums")}>
                {countLabel(account.token_count, "token", "tokens")}
              </TableCell>
              <TableCell data-label='Trusted access' className={cn(CELL, "tabular-nums")}>
                {countLabel(account.trust_policy_count, "policy", "policies")}
              </TableCell>
              <TableCell data-label='Created by' className={CELL}>
                {account.created_by?.label ?? (
                  <span className='text-muted-foreground'>Unknown</span>
                )}
              </TableCell>
              <TableCell data-label='Status' className={CELL}>
                <ServiceAccountStatusBadge account={account} />
              </TableCell>
              <TableCell className={cn(CELL, "text-right")}>
                <ServiceAccountActions
                  orgId={orgId}
                  orgSlug={orgSlug}
                  account={account}
                  takenNames={takenNames}
                />
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </TableWrapper>
  );
}
