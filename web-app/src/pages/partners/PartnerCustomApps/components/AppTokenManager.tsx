import { Check, Copy, KeyRound, Plus } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Button } from "@/components/ui/shadcn/button";
import { Card, CardContent } from "@/components/ui/shadcn/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { Table, TableBody, TableCell, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import {
  useCreateAppToken,
  usePartnerAppTokens,
  usePartnerOrgApps,
  usePartnerOrgs,
  useRevokeAppToken
} from "@/hooks/api/partners";
import { ADMIN_HEADER_ROW_CLASS, AdminTh } from "@/pages/admin/components/AdminTable";
import type { PartnerCreatedToken } from "@/types/partners";

/**
 * Mint app-scoped publish tokens for a client's app. Pick a client, then an app;
 * each token is confined to that one app at publish time (and only works while the
 * client keeps partner publishing on), so it's a safe long-lived CI credential.
 */
export default function AppTokenManager({ partnerId }: { partnerId: string }) {
  const { data: clients } = usePartnerOrgs(partnerId);
  const [clientId, setClientId] = useState<string>();
  const { data: apps } = usePartnerOrgApps(partnerId, clientId, !!clientId);
  const [appId, setAppId] = useState<string>();

  return (
    <Card>
      <CardContent className='space-y-4 p-4'>
        <div>
          <h2 className='font-semibold text-sm'>App tokens</h2>
          <p className='mt-0.5 text-muted-foreground text-xs'>
            A long-lived CI credential scoped to <b>one</b> client app — it can publish to that app
            only, and only while the client keeps partner publishing on. Prefer trusted publishing
            above when you can; use a token when OIDC isn't an option.
          </p>
        </div>

        <div className='flex flex-wrap gap-2'>
          <Select
            value={clientId}
            onValueChange={(v) => {
              setClientId(v);
              setAppId(undefined);
            }}
          >
            <SelectTrigger className='w-52' data-testid='partner-tokens-client'>
              <SelectValue placeholder='Select a client…' />
            </SelectTrigger>
            <SelectContent className={DESIGN_REFERENCE}>
              {(clients ?? []).map((c) => (
                <SelectItem
                  key={c.org_id}
                  value={c.org_id}
                  data-testid={`partner-tokens-client-${c.slug}`}
                >
                  {c.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>

          {clientId && (
            <Select value={appId} onValueChange={setAppId}>
              <SelectTrigger className='w-52' data-testid='partner-tokens-app'>
                <SelectValue placeholder='Select an app…' />
              </SelectTrigger>
              <SelectContent className={DESIGN_REFERENCE}>
                {(apps ?? []).length === 0 ? (
                  <div className='px-2 py-1.5 text-muted-foreground text-xs'>
                    No apps in this client.
                  </div>
                ) : (
                  (apps ?? []).map((a) => (
                    <SelectItem
                      key={a.id}
                      value={a.id}
                      data-testid={`partner-tokens-app-${a.slug}`}
                    >
                      {a.name}
                    </SelectItem>
                  ))
                )}
              </SelectContent>
            </Select>
          )}
        </div>

        {appId ? (
          <TokenPanel partnerId={partnerId} appId={appId} />
        ) : (
          <p className='text-muted-foreground text-xs'>
            Pick a client and app to manage its tokens.
          </p>
        )}
      </CardContent>
    </Card>
  );
}

function TokenPanel({ partnerId, appId }: { partnerId: string; appId: string }) {
  const { data: tokens, isLoading } = usePartnerAppTokens(partnerId, appId);
  const create = useCreateAppToken(partnerId, appId);
  const revoke = useRevokeAppToken(partnerId, appId);
  const [created, setCreated] = useState<PartnerCreatedToken | null>(null);

  return (
    <div className='space-y-3'>
      <Button
        size='sm'
        disabled={create.isPending}
        onClick={() => create.mutate(undefined, { onSuccess: setCreated })}
        data-testid='partner-tokens-create'
      >
        <Plus className='size-4' />
        Create token
      </Button>

      {isLoading ? (
        <Skeleton className='h-16 w-full' />
      ) : !tokens?.length ? (
        <div className='flex flex-col items-center gap-1 py-6 text-muted-foreground'>
          <KeyRound className='size-6' />
          <p className='text-xs'>No tokens for this app yet.</p>
        </div>
      ) : (
        <Table>
          <TableHeader>
            <TableRow className={ADMIN_HEADER_ROW_CLASS}>
              <AdminTh>Name</AdminTh>
              <AdminTh>Prefix</AdminTh>
              <AdminTh>Expires</AdminTh>
              <AdminTh align='right'>Actions</AdminTh>
            </TableRow>
          </TableHeader>
          <TableBody>
            {tokens.map((t) => (
              <TableRow
                key={t.id}
                className='border-border-subtle'
                data-testid={`partner-tokens-row-${t.id}`}
              >
                <TableCell className='font-medium text-sm'>{t.name}</TableCell>
                <TableCell className='font-mono text-muted-foreground text-xs'>
                  {t.token_prefix}…
                </TableCell>
                <TableCell className='text-muted-foreground text-xs tabular-nums'>
                  {t.expires_at ? new Date(t.expires_at).toLocaleDateString() : "never"}
                </TableCell>
                <TableCell className='text-right'>
                  <Button
                    variant='ghost'
                    size='sm'
                    disabled={revoke.isPending}
                    onClick={() => revoke.mutate(t.id)}
                    data-testid={`partner-tokens-revoke-${t.id}`}
                  >
                    Revoke
                  </Button>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}

      <CreatedTokenDialog token={created} onClose={() => setCreated(null)} />
    </div>
  );
}

/** The plaintext is shown once, on create, and never again. */
function CreatedTokenDialog({
  token,
  onClose
}: {
  token: PartnerCreatedToken | null;
  onClose: () => void;
}) {
  const [copied, setCopied] = useState(false);

  const copy = async () => {
    if (!token) return;
    try {
      await navigator.clipboard.writeText(token.token);
      setCopied(true);
      toast.success("Token copied");
      setTimeout(() => setCopied(false), 1500);
    } catch {
      toast.error("Couldn't copy to clipboard");
    }
  };

  return (
    <Dialog open={!!token} onOpenChange={(o) => !o && onClose()}>
      <DialogContent className={`${DESIGN_REFERENCE} max-w-lg`}>
        <DialogHeader>
          <DialogTitle>Token created</DialogTitle>
          <DialogDescription>
            Copy it now — it's shown once and can't be retrieved later. Set it as the{" "}
            <span className='font-mono'>OXY_TOKEN</span> secret in your CI.
          </DialogDescription>
        </DialogHeader>
        <div className='flex items-center gap-2'>
          <code className='flex-1 overflow-x-auto border border-border-subtle bg-field px-2 py-1.5 font-mono text-xs'>
            {token?.token}
          </code>
          <Button
            variant='outline'
            size='icon'
            onClick={copy}
            aria-label='Copy token'
            data-testid='partner-tokens-copy'
          >
            {copied ? <Check className='size-4' /> : <Copy className='size-4' />}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
