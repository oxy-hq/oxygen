import { History } from "lucide-react";
import { useState } from "react";
import TableWrapper from "@/components/settings/components/TableWrapper";
import LegacyBadge from "@/components/settings/SettingsDialog/components/LegacyBadge";
import { toTokenSummary } from "@/components/settings/SettingsDialog/sections/account/PersonalTokens/accessSummary";
import ActivityDrawer from "@/components/settings/SettingsDialog/sections/workspace/ApiKeys/ApiKeyTable/ApiKeyRow/components/ActivityDrawer";
import { Button } from "@/components/ui/shadcn/button";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from "@/components/ui/shadcn/table";
import { useOrgInventoryEndpoints } from "@/hooks/api/orgApiAccess";
import { cn } from "@/libs/shadcn/utils";
import type { InventoryToken } from "@/types/orgApiAccess";
import { TokenLifecycleCell } from "../../shared/TokenLifecycleCell";
import { lastUsedText } from "../../utils/tokens";
import { LEGACY_REVOKE_TOOLTIP } from "../inventory";
import { InventoryOwner } from "./InventoryOwner";
import { LegacyRevokeAction } from "./LegacyRevokeAction";

const CELL = "px-3 py-2.5 align-top max-md:px-0 max-md:py-0";

interface LegacyKeyGroupProps {
  orgId: string;
  orgName: string;
  /** Legacy API keys only: the caller has already taken the tokens out. */
  legacyKeys: InventoryToken[];
}

/**
 * The legacy API keys that reach an org, in a group of their own below the token list. It has
 * fewer columns than the token table because a legacy API key has less to say: it always reaches
 * what its owner does, and no policy applies to it.
 */
export function LegacyKeyGroup({ orgId, orgName, legacyKeys }: LegacyKeyGroupProps) {
  return (
    <section
      className='flex flex-col gap-3 border-t pt-4'
      aria-labelledby='api-access-legacy-keys-heading'
      data-testid='api-access-legacy-keys'
    >
      <div className='flex flex-col gap-1'>
        <h4 id='api-access-legacy-keys-heading' className='font-medium text-sm'>
          Legacy API keys
        </h4>
        <p className='max-w-xl text-muted-foreground text-xs leading-relaxed'>
          The older kind of key. Each one reaches everything its owner can in {orgName}, and no rule
          on the Policy tab applies to it. Only its owner can revoke it.
        </p>
      </div>

      <TableWrapper>
        <Table className='text-xs' data-testid='api-access-legacy-keys-table'>
          <TableHeader>
            <TableRow>
              <TableHead className='px-3'>Owner</TableHead>
              <TableHead className='px-3'>Key</TableHead>
              <TableHead className='px-3'>Status</TableHead>
              <TableHead className='px-3'>Last used</TableHead>
              <TableHead className='px-3'>
                <span className='sr-only'>Actions</span>
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {legacyKeys.map((legacyKey) => (
              <LegacyKeyInventoryRow
                key={legacyKey.id}
                orgId={orgId}
                orgName={orgName}
                legacyKey={legacyKey}
              />
            ))}
          </TableBody>
        </Table>
      </TableWrapper>
    </section>
  );
}

interface LegacyKeyInventoryRowProps {
  orgId: string;
  orgName: string;
  legacyKey: InventoryToken;
}

/** One legacy API key: badged Legacy, Activity open, Revoke disabled with the reason. */
function LegacyKeyInventoryRow({ orgId, orgName, legacyKey }: LegacyKeyInventoryRowProps) {
  const [activityOpen, setActivityOpen] = useState(false);
  const endpoints = useOrgInventoryEndpoints(orgId);
  const summary = toTokenSummary(legacyKey);

  return (
    <TableRow
      className={cn(legacyKey.status === "revoked" && "text-muted-foreground")}
      data-testid='api-access-legacy-key-row'
      data-token-name={legacyKey.name}
      data-token-kind={legacyKey.kind}
    >
      <TableCell data-label='Owner' className={cn(CELL, "whitespace-normal")}>
        <InventoryOwner owner={legacyKey.owner} />
      </TableCell>
      <TableCell data-label='Key' className={cn(CELL, "whitespace-normal")}>
        <p className='flex items-center gap-2'>
          <span className='font-medium text-foreground'>{legacyKey.name}</span>
          <LegacyBadge />
        </p>
        <p className='font-mono text-muted-foreground'>{summary.masked_key}</p>
      </TableCell>
      <TableCell data-label='Status' className={CELL}>
        <TokenLifecycleCell token={legacyKey} testId='api-access-legacy-key-status' />
      </TableCell>
      <TableCell data-label='Last used' className={CELL}>
        {lastUsedText(legacyKey.last_used_at)}
      </TableCell>
      <TableCell className={cn(CELL, "text-right")}>
        <div className='flex items-center justify-end gap-0.5'>
          <Button
            variant='ghost'
            size='icon'
            className='size-7 text-muted-foreground'
            onClick={() => setActivityOpen(true)}
            aria-label={`Activity for ${legacyKey.name}`}
            title='Activity in this organization'
            data-testid='api-access-legacy-key-activity'
          >
            <History className='size-3.5' aria-hidden />
          </Button>
          {legacyKey.status === "revoked" ? (
            // Holds the column's width so the Activity buttons stay aligned down the table.
            <span className='inline-block size-7' aria-hidden />
          ) : (
            <LegacyRevokeAction name={legacyKey.name} reason={LEGACY_REVOKE_TOOLTIP} />
          )}
        </div>

        <ActivityDrawer
          token={summary}
          endpoints={endpoints}
          scopeNote={`Only what this legacy API key did in ${orgName}. Its use elsewhere isn't shown.`}
          usageNote="Request counts aren't shown for a person's legacy API key: they include its use in other organizations."
          open={activityOpen}
          onOpenChange={setActivityOpen}
        />
      </TableCell>
    </TableRow>
  );
}
