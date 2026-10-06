import { KeyRound, Plus } from "lucide-react";
import { useState } from "react";
import TableWrapper from "@/components/settings/components/TableWrapper";
import { Button } from "@/components/ui/shadcn/button";
import { Table, TableBody, TableHead, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import { useServiceAccountTokens, useTokenPolicy } from "@/hooks/api/orgApiAccess";
import type { TokenWithSecret } from "@/types/apiToken";
import type { ServiceAccount } from "@/types/orgApiAccess";
import type { Organization } from "@/types/organization";
import { EmptyState, ListState } from "../../../shared/ListState";
import { TokenSecretDialog } from "../../../shared/TokenSecretDialog";
import { CreateTokenDialog } from "./CreateTokenDialog";
import { PaneHeader } from "./PaneHeader";
import { TokenRow } from "./TokenRow";

interface TokensPaneProps {
  org: Organization;
  account: ServiceAccount;
}

/** The account's `oxy_sat_` tokens: long-lived secrets for places that can't use trusted access. */
export function TokensPane({ org, account }: TokensPaneProps) {
  const { data, isPending, error, refetch } = useServiceAccountTokens(org.id, account.id);
  // Unknown (still loading, or a server without policies) reads as "no cap":
  // the server is the one that enforces it, so the worst case is its own refusal.
  const { data: policy } = useTokenPolicy(org.id);
  const maxLifetimeDays = policy?.max_lifetime_days ?? null;

  const [creating, setCreating] = useState(false);
  const [minted, setMinted] = useState<{
    secret: TokenWithSecret;
    verb: "created" | "regenerated";
  }>();
  const tokens = data ?? [];
  const disabled = account.disabled_at !== null;

  const createButton = (
    <Button
      size='sm'
      variant='outline'
      onClick={() => setCreating(true)}
      disabled={disabled}
      title={disabled ? "Enable the account to create a token" : undefined}
      data-testid='api-access-token-create'
    >
      <Plus className='size-4' aria-hidden />
      Create token
    </Button>
  );

  return (
    <section className='flex flex-col gap-3' data-testid='api-access-tokens'>
      <PaneHeader
        title='Tokens'
        description='A secret that signs in as this account until it expires or is revoked. Use one where trusted access can’t reach: a script, a server, a CI system other than GitHub Actions.'
        action={tokens.length > 0 ? createButton : undefined}
      />

      <ListState
        what='tokens'
        isPending={isPending}
        // A failed background refetch keeps showing what was already loaded.
        error={data ? null : error}
        isEmpty={tokens.length === 0}
        onRetry={refetch}
        testId='api-access-tokens'
        empty={
          <EmptyState
            icon={KeyRound}
            title='No tokens'
            action={createButton}
            testId='api-access-tokens-empty'
          >
            Nothing can sign in as this account with a secret. If its work runs in GitHub Actions,
            trusted access below needs no token at all.
          </EmptyState>
        }
      >
        <TableWrapper>
          <Table className='text-xs' data-testid='api-access-token-table'>
            <TableHeader>
              <TableRow>
                <TableHead className='px-3'>Name</TableHead>
                <TableHead className='px-3'>Access</TableHead>
                <TableHead className='px-3'>Status</TableHead>
                <TableHead className='px-3'>Last used</TableHead>
                <TableHead className='px-3'>
                  <span className='sr-only'>Actions</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {tokens.map((token) => (
                <TokenRow
                  key={token.id}
                  orgId={org.id}
                  saId={account.id}
                  token={token}
                  allowNoExpiry={maxLifetimeDays === null}
                  onRegenerated={(secret) => setMinted({ secret, verb: "regenerated" })}
                />
              ))}
            </TableBody>
          </Table>
        </TableWrapper>
      </ListState>

      <CreateTokenDialog
        open={creating}
        onOpenChange={setCreating}
        orgId={org.id}
        account={account}
        maxLifetimeDays={maxLifetimeDays}
        onCreated={(secret) => setMinted({ secret, verb: "created" })}
      />
      <TokenSecretDialog
        minted={minted?.secret ?? null}
        verb={minted?.verb ?? "created"}
        onClose={() => setMinted(undefined)}
      />
    </section>
  );
}
