import { Ban, FileCode2, KeyRound, MoreHorizontal, Pencil, Play, Plus, Trash2 } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import TableWrapper from "@/components/settings/components/TableWrapper";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger
} from "@/components/ui/shadcn/dropdown-menu";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from "@/components/ui/shadcn/table";
import {
  useDeleteIssuerTrustPolicy,
  useDeleteOidcIssuer,
  useIssuerTrustPolicies,
  useOidcIssuers,
  useServiceAccounts,
  useUpdateIssuerTrustPolicy
} from "@/hooks/api/orgApiAccess";
import { cn } from "@/libs/shadcn/utils";
import type { IssuerTrustPolicy, OidcIssuer } from "@/types/orgApiAccess";
import type { Organization } from "@/types/organization";
import { PaneHeader } from "../ServiceAccounts/Detail/components/PaneHeader";
import { AccessSummary } from "../shared/AccessSummary";
import { ConfirmDialog } from "../shared/ConfirmDialog";
import { EmptyState, ListState } from "../shared/ListState";
import { describeApiError } from "../utils/errors";
import { describeAccess } from "../utils/grants";
import {
  awsRoleOf,
  describeConditions,
  describeKeySource,
  PRESETS,
  presetOf
} from "../utils/issuerTrustForm";
import { lastUsedText } from "../utils/tokens";
import { IssuerDialog } from "./IssuerDialog";
import { ClientSetupDialog, TrustPolicyDialog } from "./TrustPolicyDialog";

const CELL = "px-3 py-2.5 align-top max-md:px-0 max-md:py-0";

/**
 * Trusted access from outside Oxygen, in one list: the OIDC issuers this org
 * trusts, and under each, its trust policies — which of its tokens, by their
 * claims, may act as which service account. A service such as ferret on ECS
 * signs in with the token its platform gives it, and no key is stored.
 */
export function OidcIssuers({ org }: { org: Organization }) {
  const issuers = useOidcIssuers(org.id);
  const policies = useIssuerTrustPolicies(org.id);
  const { data: accounts } = useServiceAccounts(org.id);
  const [registering, setRegistering] = useState(false);
  const [policyFor, setPolicyFor] = useState<OidcIssuer | null>(null);
  const [editing, setEditing] = useState<IssuerTrustPolicy | null>(null);
  const [setup, setSetup] = useState<{ policy: IssuerTrustPolicy; justCreated: boolean } | null>(
    null
  );
  const list = issuers.data ?? [];

  const registerButton = (
    <Button
      size='sm'
      variant='outline'
      onClick={() => setRegistering(true)}
      data-testid='api-access-issuer-create'
    >
      <Plus className='size-4' aria-hidden />
      Register issuer
    </Button>
  );

  return (
    <section className='flex flex-col gap-3' data-testid='api-access-issuers'>
      <PaneHeader
        title='Trusted access'
        description='Services outside Oxygen that sign in with a token their platform issues, such as an AWS workload. Register the issuer, then add a trust policy saying which of its tokens may act as a service account.'
        action={list.length > 0 ? registerButton : undefined}
      />

      <ListState
        what='OIDC issuers'
        isPending={issuers.isPending}
        error={issuers.data ? null : issuers.error}
        isEmpty={list.length === 0}
        onRetry={issuers.refetch}
        testId='api-access-issuers'
        empty={
          <EmptyState
            icon={KeyRound}
            title='No issuers registered'
            action={registerButton}
            testId='api-access-issuers-empty'
          >
            Register an AWS account to let one of its IAM roles act as a service account with the
            token STS signs for it, or any other OIDC issuer with Custom OIDC.
          </EmptyState>
        }
      >
        <div className='flex flex-col gap-4'>
          {list.map((issuer) => (
            <IssuerBlock
              key={issuer.id}
              orgId={org.id}
              issuer={issuer}
              policies={(policies.data ?? []).filter((p) => p.issuer.id === issuer.id)}
              onAddPolicy={() => setPolicyFor(issuer)}
              onEdit={setEditing}
              onShowSetup={(policy) => setSetup({ policy, justCreated: false })}
            />
          ))}
        </div>
      </ListState>

      <IssuerDialog open={registering} onOpenChange={setRegistering} orgId={org.id} />
      <TrustPolicyDialog
        open={policyFor !== null || editing !== null}
        onOpenChange={(open) => {
          if (!open) {
            setPolicyFor(null);
            setEditing(null);
          }
        }}
        orgId={org.id}
        issuer={policyFor}
        accounts={accounts ?? []}
        policy={editing}
        onCreated={(policy) => {
          toast.success("Added the trust policy");
          if (awsRoleOf(policy.conditions)) setSetup({ policy, justCreated: true });
        }}
      />
      <ClientSetupDialog
        policy={setup?.policy ?? null}
        justCreated={setup?.justCreated ?? false}
        orgSlug={org.slug}
        onClose={() => setSetup(null)}
      />
    </section>
  );
}

function IssuerBlock({
  orgId,
  issuer,
  policies,
  onAddPolicy,
  onEdit,
  onShowSetup
}: {
  orgId: string;
  issuer: OidcIssuer;
  policies: IssuerTrustPolicy[];
  onAddPolicy: () => void;
  onEdit: (policy: IssuerTrustPolicy) => void;
  onShowSetup: (policy: IssuerTrustPolicy) => void;
}) {
  const remove = useDeleteOidcIssuer();
  const [confirming, setConfirming] = useState(false);
  const preset = PRESETS.find((p) => p.value === presetOf(issuer));

  const handleDelete = async () => {
    try {
      await remove.mutateAsync({ orgId, issuerId: issuer.id });
      toast.success("Removed the issuer");
      setConfirming(false);
    } catch (err) {
      toast.error(describeApiError(err, "Couldn't remove the issuer."));
    }
  };

  return (
    <div
      className='flex flex-col gap-3 rounded-md border p-3'
      data-testid='api-access-issuer'
      data-issuer={issuer.issuer}
    >
      <div className='flex flex-wrap items-start justify-between gap-2'>
        <div className='flex min-w-0 flex-col gap-0.5 text-xs'>
          <p className='flex items-center gap-2 font-medium text-foreground'>
            {issuer.name}
            {preset && (
              <Badge variant='outline' className='font-normal text-muted-foreground'>
                {preset.label}
              </Badge>
            )}
          </p>
          <p className='break-all font-mono text-muted-foreground'>{issuer.issuer}</p>
          <p className='break-all text-muted-foreground'>
            {describeKeySource(issuer)}
            {issuer.jti_single_use ? " Each token is exchanged once." : ""}
          </p>
        </div>
        <div className='flex items-center gap-1'>
          <Button
            size='sm'
            variant='outline'
            onClick={onAddPolicy}
            data-testid='api-access-trust-policy-create'
          >
            <Plus className='size-4' aria-hidden />
            Add trust policy
          </Button>
          <Button
            variant='ghost'
            size='icon'
            className='size-8 text-muted-foreground'
            aria-label={`Remove ${issuer.name}`}
            onClick={() => setConfirming(true)}
            data-testid='api-access-issuer-delete'
          >
            <Trash2 className='size-4' aria-hidden />
          </Button>
        </div>
      </div>

      {policies.length === 0 ? (
        <p className='text-muted-foreground text-xs' data-testid='api-access-issuer-no-policies'>
          No trust policy yet: no token from this issuer can act as anything.
        </p>
      ) : (
        <TableWrapper>
          <Table className='text-xs' data-testid='api-access-trust-policy-table'>
            <TableHeader>
              <TableRow>
                <TableHead className='px-3'>Acts as</TableHead>
                <TableHead className='px-3'>When the token says</TableHead>
                <TableHead className='px-3'>Access</TableHead>
                <TableHead className='px-3'>Last used</TableHead>
                <TableHead className='w-10 px-3'>
                  <span className='sr-only'>Actions</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {policies.map((policy) => (
                <TrustPolicyRow
                  key={policy.id}
                  orgId={orgId}
                  policy={policy}
                  onEdit={onEdit}
                  onShowSetup={onShowSetup}
                />
              ))}
            </TableBody>
          </Table>
        </TableWrapper>
      )}

      <ConfirmDialog
        open={confirming}
        onOpenChange={setConfirming}
        title={`Remove ${issuer.name}?`}
        confirmLabel='Remove'
        cancelLabel='Keep issuer'
        destructive
        isPending={remove.isPending}
        onConfirm={handleDelete}
        testId='api-access-issuer-delete-dialog'
      >
        <p>
          {issuer.policy_count === 0
            ? "No service account trusts it yet."
            : `Its ${issuer.policy_count} ${issuer.policy_count === 1 ? "trust policy is" : "trust policies are"} deleted with it, and every token they minted stops working now.`}
        </p>
      </ConfirmDialog>
    </div>
  );
}

function TrustPolicyRow({
  orgId,
  policy,
  onEdit,
  onShowSetup
}: {
  orgId: string;
  policy: IssuerTrustPolicy;
  onEdit: (policy: IssuerTrustPolicy) => void;
  onShowSetup: (policy: IssuerTrustPolicy) => void;
}) {
  const update = useUpdateIssuerTrustPolicy();
  const remove = useDeleteIssuerTrustPolicy();
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const disabled = policy.disabled_at !== null;
  const ref = { orgId, saId: policy.service_account.id, policyId: policy.id };
  const role = awsRoleOf(policy.conditions);

  const setDisabled = async (next: boolean) => {
    try {
      await update.mutateAsync({ ...ref, request: { disabled: next } });
      toast.success(next ? "Disabled the policy" : "Enabled the policy");
    } catch (err) {
      toast.error(describeApiError(err, `Couldn't ${next ? "disable" : "enable"} the policy.`));
    }
  };

  const handleDelete = async () => {
    try {
      await remove.mutateAsync(ref);
      toast.success("Deleted the policy");
      setConfirmingDelete(false);
    } catch (err) {
      toast.error(describeApiError(err, "Couldn't delete the policy."));
    }
  };

  return (
    <TableRow
      className={cn(disabled && "text-muted-foreground")}
      data-testid='api-access-trust-policy-row'
    >
      <TableCell data-label='Acts as' className={cn(CELL, "whitespace-normal")}>
        <p className='font-medium text-foreground'>{policy.service_account.name}</p>
        {disabled && (
          <Badge variant='outline' className='mt-1 text-muted-foreground'>
            Disabled
          </Badge>
        )}
      </TableCell>
      <TableCell data-label='When the token says' className={cn(CELL, "whitespace-normal")}>
        <p className='break-all font-mono'>{role ?? describeConditions(policy.conditions)}</p>
      </TableCell>
      <TableCell data-label='Access' className={cn(CELL, "whitespace-normal")}>
        <AccessSummary access={describeAccess(policy.grants)} />
      </TableCell>
      <TableCell data-label='Last used' className={CELL}>
        {lastUsedText(policy.last_used_at)}
      </TableCell>
      <TableCell className={cn(CELL, "text-right")}>
        <DropdownMenu modal={false}>
          <DropdownMenuTrigger asChild>
            <Button
              variant='ghost'
              size='icon'
              className='size-7 text-muted-foreground'
              aria-label={`Actions for the policy acting as ${policy.service_account.name}`}
              data-testid='api-access-trust-policy-actions'
            >
              <MoreHorizontal className='size-4' aria-hidden />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align='end'>
            {role && (
              <DropdownMenuItem className='text-xs' onSelect={() => onShowSetup(policy)}>
                <FileCode2 aria-hidden />
                Show workload setup
              </DropdownMenuItem>
            )}
            <DropdownMenuItem className='text-xs' onSelect={() => onEdit(policy)}>
              <Pencil aria-hidden />
              Edit access
            </DropdownMenuItem>
            <DropdownMenuItem
              className='text-xs'
              onSelect={() => setDisabled(!disabled)}
              data-testid={
                disabled ? "api-access-trust-policy-enable" : "api-access-trust-policy-disable"
              }
            >
              {disabled ? <Play aria-hidden /> : <Ban aria-hidden />}
              {disabled ? "Enable" : "Disable"}
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem
              variant='destructive'
              className='text-xs'
              onSelect={() => setConfirmingDelete(true)}
              data-testid='api-access-trust-policy-delete'
            >
              <Trash2 aria-hidden />
              Delete
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>

        <ConfirmDialog
          open={confirmingDelete}
          onOpenChange={setConfirmingDelete}
          title='Delete this trust policy?'
          confirmLabel='Delete'
          cancelLabel='Keep policy'
          destructive
          isPending={remove.isPending}
          onConfirm={handleDelete}
          testId='api-access-trust-policy-delete-dialog'
        >
          <p>
            Tokens from {policy.issuer.name} can no longer act as {policy.service_account.name}{" "}
            through it, and the Oxygen tokens it minted stop working now.
          </p>
          <p>To pause it instead, disable the policy.</p>
        </ConfirmDialog>
      </TableCell>
    </TableRow>
  );
}
