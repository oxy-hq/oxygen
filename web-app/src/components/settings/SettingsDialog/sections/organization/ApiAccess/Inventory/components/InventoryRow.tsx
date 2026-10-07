import { Ban, Bot, Clock, History, ShieldAlert } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import {
  tokenKindLabel,
  toTokenSummary
} from "@/components/settings/SettingsDialog/sections/account/PersonalTokens/accessSummary";
import ActivityDrawer from "@/components/settings/SettingsDialog/sections/workspace/ApiKeys/ApiKeyTable/ApiKeyRow/components/ActivityDrawer";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import { TableCell, TableRow } from "@/components/ui/shadcn/table";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import { useOrgInventoryEndpoints, useRevokeOrgTokenGrant } from "@/hooks/api/orgApiAccess";
import { cn } from "@/libs/shadcn/utils";
import { policyBlockReason } from "@/libs/tokenPolicy";
import type { InventoryToken } from "@/types/orgApiAccess";
import { AccessSummary } from "../../shared/AccessSummary";
import { ConfirmDialog } from "../../shared/ConfirmDialog";
import { TokenLifecycleCell } from "../../shared/TokenLifecycleCell";
import { describeApiError } from "../../utils/errors";
import { lastUsedText } from "../../utils/tokens";
import { inventoryAccess, inventoryRevokeAction } from "../inventory";
import { InventoryOwner } from "./InventoryOwner";
import { LegacyRevokeAction } from "./LegacyRevokeAction";

interface InventoryRowProps {
  orgId: string;
  orgName: string;
  token: InventoryToken;
  /** Opens a service account's page, where its tokens are managed. */
  onOpenAccount: (accountId: string) => void;
}

const CELL = "px-3 py-2.5 align-top max-md:px-0 max-md:py-0";
const ICON_BUTTON = "size-7 text-muted-foreground";

/** One API token in an org's inventory. Legacy API keys have their own row, in LegacyKeyGroup. */
export function InventoryRow({ orgId, orgName, token, onOpenAccount }: InventoryRowProps) {
  const revokeGrant = useRevokeOrgTokenGrant();
  const [activityOpen, setActivityOpen] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const endpoints = useOrgInventoryEndpoints(orgId);
  const summary = toTokenSummary(token);

  const handleRevoke = async () => {
    try {
      await revokeGrant.mutateAsync({ orgId, tokenId: token.id });
      toast.success(`Revoked ${token.name}'s access to ${orgName}`);
      setConfirming(false);
    } catch (err) {
      toast.error(describeApiError(err, `Couldn't revoke ${token.name}'s access.`));
    }
  };

  return (
    <TableRow
      className={cn(token.status === "revoked" && "text-muted-foreground")}
      data-testid='api-access-inventory-row'
      data-token-name={token.name}
      data-token-kind={token.kind}
    >
      <TableCell data-label='Owner' className={cn(CELL, "whitespace-normal")}>
        <InventoryOwner owner={token.owner} />
      </TableCell>
      <TableCell data-label='Token' className={cn(CELL, "whitespace-normal")}>
        <p className='font-medium text-foreground'>{token.name}</p>
        <p className='font-mono text-muted-foreground'>{summary.masked_key}</p>
      </TableCell>
      {/* An agent token is a personal token by kind: its source is what says "Agent". */}
      <TableCell data-label='Kind' className={CELL} data-testid='api-access-inventory-kind'>
        {tokenKindLabel(token)}
      </TableCell>
      <TableCell data-label='Access here' className={cn(CELL, "whitespace-normal")}>
        <AccessSummary access={inventoryAccess(token)} />
      </TableCell>
      <TableCell data-label='Status' className={CELL}>
        <TokenLifecycleCell token={token} testId='api-access-inventory-status' />
        {token.long_lived_while_trusted_access && <LongLivedChip />}
      </TableCell>
      <TableCell data-label='Last used' className={CELL}>
        {lastUsedText(token.last_used_at)}
      </TableCell>
      <TableCell data-label='Blocked by policy' className={cn(CELL, "whitespace-normal")}>
        <BlockedCell reason={token.blocked_by_policy} />
      </TableCell>
      <TableCell className={cn(CELL, "text-right")}>
        <div className='flex items-center justify-end gap-0.5'>
          <Button
            variant='ghost'
            size='icon'
            className={ICON_BUTTON}
            onClick={() => setActivityOpen(true)}
            aria-label={`Activity for ${token.name}`}
            title='Activity in this organization'
            data-testid='api-access-inventory-activity'
          >
            <History className='size-3.5' aria-hidden />
          </Button>
          <RevokeAction
            token={token}
            onRevoke={() => setConfirming(true)}
            onOpenAccount={onOpenAccount}
          />
        </div>

        <ConfirmDialog
          open={confirming}
          onOpenChange={setConfirming}
          title={`Revoke ${token.name}'s access to ${orgName}?`}
          confirmLabel='Revoke access'
          cancelLabel='Keep access'
          destructive
          isPending={revokeGrant.isPending}
          onConfirm={handleRevoke}
          testId='api-access-inventory-revoke-dialog'
        >
          <RevokeConsequences token={token} orgName={orgName} />
        </ConfirmDialog>

        <ActivityDrawer
          token={summary}
          endpoints={endpoints}
          scopeNote={`Only what this token did in ${orgName}. Its use elsewhere isn't shown.`}
          usageNote={
            token.owner.type === "user"
              ? "Request counts aren't shown for a person's token: they include its use in other organizations."
              : undefined
          }
          open={activityOpen}
          onOpenChange={setActivityOpen}
        />
      </TableCell>
    </TableRow>
  );
}

/**
 * What revoke-grant does, said before it happens. It can't be undone: no route lifts an org's
 * block, and the owner can't grant this org back to the same token. It costs the token this org
 * and nothing else, all-access or not: what spans organizations (chat, work, notifications)
 * goes on answering with this org left out.
 */
function RevokeConsequences({ token, orgName }: { token: InventoryToken; orgName: string }) {
  const owner = token.owner.label;
  return (
    <>
      <p>
        This ends the token's reach into {orgName}. The token itself isn't revoked, and it can't be
        given access here again: {owner} would need a new token.
      </p>
      <p>It keeps working in every other organization {owner} uses it for.</p>
      <p>{owner} is emailed, so they know why it stopped working here.</p>
    </>
  );
}

/** Blocked is the exception worth colour; the ordinary answer is a quiet "No". */
function BlockedCell({ reason }: { reason: string | null }) {
  if (!reason) return <span className='text-muted-foreground'>No</span>;
  return (
    <span
      className='inline-flex max-w-56 items-start gap-1.5 text-foreground'
      data-testid='api-access-inventory-blocked'
    >
      <ShieldAlert className='mt-0.5 size-3.5 shrink-0 text-warning' aria-hidden />
      <span>
        <span className='font-medium'>Blocked.</span> {policyBlockReason(reason)}.
      </span>
    </span>
  );
}

/**
 * A stored secret that outlives the org's move to trusted access. A nudge, not an alarm: the
 * token still works, and nothing here acts on it.
 */
function LongLivedChip() {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Badge
          variant='outline'
          className='mt-1 cursor-default border-warning/40 font-normal text-warning'
          data-testid='api-access-inventory-long-lived'
        >
          <Clock aria-hidden />
          Long-lived
        </Badge>
      </TooltipTrigger>
      <TooltipContent className='max-w-xs text-xs'>
        It never expires or has more than 90 days left, and this organization's CI already signs in
        with trusted access. If this token is a CI secret, it may no longer be needed.
      </TooltipContent>
    </Tooltip>
  );
}

function RevokeAction({
  token,
  onRevoke,
  onOpenAccount
}: {
  token: InventoryToken;
  onRevoke: () => void;
  onOpenAccount: (accountId: string) => void;
}) {
  const action = inventoryRevokeAction(token);

  switch (action.kind) {
    case "revoke_grant":
      return (
        <Button
          variant='ghost'
          size='icon'
          className={cn(ICON_BUTTON, "hover:text-destructive")}
          onClick={onRevoke}
          aria-label={`Revoke ${token.name}'s access to this organization`}
          title="Revoke this organization's access"
          data-testid='api-access-inventory-revoke'
        >
          <Ban className='size-3.5' aria-hidden />
        </Button>
      );
    case "legacy":
      // Legacy API keys are listed in their own group (LegacyKeyGroup), so no row here is one.
      // Should one arrive anyway, it still can't be revoked from this side.
      return <LegacyRevokeAction name={token.name} reason={action.reason} />;
    case "service_account":
      return (
        <Button
          variant='ghost'
          size='icon'
          className={ICON_BUTTON}
          onClick={() => onOpenAccount(action.accountId)}
          aria-label={`Open ${token.owner.label} to manage this token`}
          title='Manage in its service account'
          data-testid='api-access-inventory-open-account'
        >
          <Bot className='size-3.5' aria-hidden />
        </Button>
      );
    case "none":
      // Holds the column's width so the Activity buttons stay aligned down the table.
      return <span className='inline-block size-7' aria-hidden />;
  }
}
