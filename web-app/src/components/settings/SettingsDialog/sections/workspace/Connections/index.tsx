import type React from "react";
import { useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import { useRole } from "@/hooks/useRole";
import {
  fetchOauthAuthorizeUrl,
  OAUTH_PROVIDERS,
  type OauthProvider
} from "@/services/api/integrations";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import NoAccessNotice from "../../../components/NoAccessNotice";

/**
 * Connect a workspace to a third-party OAuth provider.
 *
 * The tokens land in the workspace secret manager under the names shown, and a
 * scheduled Oxy Function in the app trades the refresh token for access tokens
 * — the platform does NOT refresh on its own. See
 * `internal-docs/customer-apps-integrations.md`.
 */
const ProviderCard: React.FC<{ provider: OauthProvider; projectId: string }> = ({
  provider,
  projectId
}) => {
  const [clientId, setClientId] = useState("");
  const [clientSecret, setClientSecret] = useState("");
  const [connecting, setConnecting] = useState(false);

  const connect = async () => {
    if (!clientId.trim()) {
      toast.error("Client ID is required");
      return;
    }
    setConnecting(true);
    try {
      const url = await fetchOauthAuthorizeUrl(projectId, provider.slug, {
        client_id: clientId.trim(),
        // Omitted rather than sent blank on a reconnect, so the stored secret
        // is reused instead of being overwritten with an empty string.
        client_secret: clientSecret.trim() || undefined,
        client_secret_var: provider.clientSecretVar,
        refresh_token_var: provider.refreshTokenVar,
        mode: "redirect",
        return_path: window.location.href
      });
      // Full-page redirect rather than a popup: popups are blocked often enough
      // that a settings flow silently doing nothing is the worse failure, and
      // there is no realm id to hand back through postMessage here.
      window.location.assign(url);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : "Could not start the connection");
      setConnecting(false);
    }
  };

  return (
    <div className='flex flex-col gap-3 rounded-md border border-border p-4'>
      <div>
        <p className='font-medium'>{provider.label}</p>
        <p className='text-muted-foreground text-sm'>{provider.grants}</p>
      </div>
      <div className='flex flex-col gap-2'>
        <Label htmlFor={`${provider.slug}-client-id`}>Client ID</Label>
        <Input
          id={`${provider.slug}-client-id`}
          value={clientId}
          onChange={(e) => setClientId(e.target.value)}
          placeholder='xxxxx.apps.googleusercontent.com'
        />
      </div>
      <div className='flex flex-col gap-2'>
        <Label htmlFor={`${provider.slug}-client-secret`}>Client secret</Label>
        <Input
          id={`${provider.slug}-client-secret`}
          type='password'
          value={clientSecret}
          onChange={(e) => setClientSecret(e.target.value)}
          placeholder='Leave blank to reuse the stored secret'
        />
      </div>
      <p className='text-muted-foreground text-xs'>
        Stored as <code>{provider.clientSecretVar}</code>; the refresh token lands in{" "}
        <code>{provider.refreshTokenVar}</code>. Your app needs a scheduled function to trade that
        for access tokens — the platform does not refresh on its own.
      </p>
      <div>
        <Button onClick={connect} disabled={connecting}>
          {connecting ? "Redirecting…" : `Connect ${provider.label}`}
        </Button>
      </div>
    </div>
  );
};

const Connections: React.FC = () => {
  const { workspace } = useCurrentWorkspace();
  // Denies only a role we KNOW is not admin, rather than using
  // `CanWorkspaceAdmin`, which treats `undefined` as deny.
  //
  // The sibling sections get away with that because a user only reaches them by
  // clicking, long after `/details` has resolved. This one is different: it is
  // the single place in the app where the settings dialog opens WITHOUT a
  // click — `useOauthConnectReturn` opens it from a mount-time effect on the
  // OAuth return. So the admin who just successfully connected would be shown
  // "you need workspace admin access" as the last frame of a success flow,
  // until the fetch lands.
  const { workspace: wsRole, is } = useRole();

  if (wsRole !== undefined && !is.workspaceAdmin) {
    return <NoAccessNotice>You need workspace admin access to manage connections.</NoAccessNotice>;
  }

  return (
    <div className='flex flex-col gap-5'>
      {!workspace?.id ? null : (
        <div className='flex flex-col gap-4'>
          {OAUTH_PROVIDERS.map((p) => (
            <ProviderCard key={p.slug} provider={p} projectId={workspace.id} />
          ))}
        </div>
      )}
    </div>
  );
};

export default Connections;
