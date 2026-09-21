import { KeyRound, Loader2, Plus } from "lucide-react";
import { type FormEvent, useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { Card, CardContent } from "@/components/ui/shadcn/card";
import { Input } from "@/components/ui/shadcn/input";
import {
  useCreatePublishToken,
  usePublishTokens,
  useRevokePublishToken
} from "@/hooks/api/publishTokens/usePublishTokens";
import type { CreatedPublishToken, PublishToken } from "@/types/publishTokens";
import { AdminAsync } from "../components/AdminAsync";
import { AdminEmptyState } from "../components/AdminEmptyState";
import { AdminPage } from "../components/AdminPage";
import CiInstructions from "./components/CiInstructions";
import { CreatedTokenDialog } from "./components/CreatedTokenDialog";
import { PublishTokenRow } from "./components/PublishTokenRow";
import { RevokeTokenDialog } from "./components/RevokeTokenDialog";

/**
 * `/admin/publish-tokens` — manage **App publish tokens**: long-lived
 * bearer credentials for machine auth (primarily `oxyc publish` in CI),
 * a stable replacement for the ~7-day session JWT.
 *
 * Open to any Global Admin (the `app_admins` table). A live token acts as
 * its minting admin **only on the customer-apps publish surface** — it
 * cannot delete apps, mint app API keys, or manage tokens (see the
 * `app_publish_token_scope` middleware). Tokens are managed across admins:
 * anyone here can revoke anyone's token.
 */
const DESCRIPTION = (
  <>
    Long-lived bearer tokens for machine auth — set one as the{" "}
    <span className='font-mono'>OXY_TOKEN</span> secret so{" "}
    <span className='font-mono'>oxyc publish</span> works in CI without an expiring login. A token
    can publish and read the custom-apps surface only; it can't delete apps, mint app API keys, or
    manage tokens.
  </>
);

export default function AdminPublishTokens() {
  // Deliberately the whole query, not `data: tokens = []`: that default made a failed
  // fetch render as "No publish tokens yet.", which reads as "your CI credential was
  // revoked" on the one page that would tell you otherwise.
  const tokens = usePublishTokens();
  const create = useCreatePublishToken();
  const revoke = useRevokePublishToken();
  const [name, setName] = useState("");
  const [created, setCreated] = useState<CreatedPublishToken | null>(null);
  const [pendingRevoke, setPendingRevoke] = useState<PublishToken | null>(null);

  const onSubmit = (e: FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    const trimmed = name.trim();
    create.mutate(trimmed, {
      onSuccess: (token) => {
        setCreated(token);
        setName("");
      }
    });
  };

  const confirmRevoke = () => {
    if (!pendingRevoke) return;
    revoke.mutate(pendingRevoke.id, {
      onSettled: () => setPendingRevoke(null)
    });
  };

  const body = (
    <>
      <Card className='mb-6'>
        <CardContent className='p-4'>
          <form onSubmit={onSubmit} className='flex flex-col gap-3 sm:flex-row sm:items-center'>
            <div className='relative flex-1'>
              <Input
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder='Token name (e.g. ci-publish)'
                disabled={create.isPending}
                autoComplete='off'
              />
            </div>
            <Button type='submit' disabled={create.isPending || !name.trim()}>
              {create.isPending ? (
                <>
                  <Loader2 className='size-4 animate-spin' />
                  Creating…
                </>
              ) : (
                <>
                  <Plus className='size-4' />
                  Create token
                </>
              )}
            </Button>
          </form>
        </CardContent>
      </Card>

      <CiInstructions />

      <AdminAsync
        query={tokens}
        noun='publish tokens'
        rows={3}
        isEmpty={(rows) => rows.length === 0}
        empty={
          <AdminEmptyState
            icon={KeyRound}
            title='No publish tokens yet.'
            description='Create one above to authenticate `oxyc publish` from CI.'
          />
        }
      >
        {(rows) => (
          <Card>
            <CardContent className='p-0'>
              <ul className='divide-y divide-border'>
                {rows.map((token) => (
                  <PublishTokenRow key={token.id} token={token} onRevoke={setPendingRevoke} />
                ))}
              </ul>
            </CardContent>
          </Card>
        )}
      </AdminAsync>

      <CreatedTokenDialog token={created} onClose={() => setCreated(null)} />

      <RevokeTokenDialog
        token={pendingRevoke}
        isRevoking={revoke.isPending}
        onOpenChange={(open) => {
          if (!open && !revoke.isPending) setPendingRevoke(null);
        }}
        onConfirm={confirmRevoke}
      />
    </>
  );

  // Embedded, this page IS the "Tokens" tab inside Custom apps: that surface already
  // owns the frame and the heading, so no AdminPage here — the branch is unchanged.
  // `space-y-0`: the create-token card and CiInstructions carry their own `mb-6`, so
  // the kit's rhythm would stack on top of it.
  //
  // There used to be an `embedded` branch above this, for when these tokens rendered
  // as a tab inside Custom apps. That tab is gone and nothing embeds this any more, so
  // the prop went with it rather than staying as a dead branch that still runs in CI.
  // The page is reached from the app console's Publishing & CI panel.
  return (
    <AdminPage
      width='narrow'
      bodyClassName='space-y-0'
      description={DESCRIPTION}
      data-testid='admin-publish-tokens'
    >
      {body}
    </AdminPage>
  );
}
