import { useEffect, useState } from "react";
import { toast } from "sonner";
import { useQuickBooksConnect } from "@/hooks/api/quickbooks/useQuickBooksConnect";
import { QB_WIZARD_STASH_KEY } from "@/services/api/quickbooks";

export interface QuickBooksScaffoldConfig {
  clientId: string;
  clientSecretVar: string;
  refreshTokenVar: string;
  realmId: string;
  baseUrl?: string;
}

/** Shared dialog fields restored from a stashed wizard after an OAuth redirect. */
interface QbRestoredFields {
  destinationDb: string | null;
  name: string;
  description: string;
}

interface UseQuickBooksCredentialsArgs {
  open: boolean;
  projectId: string;
  /** Re-applies the shared dialog fields (destination/name/description/step)
   *  once a stashed wizard is restored after a full-page OAuth redirect. */
  onRestore: (fields: QbRestoredFields) => void;
  onError: (message: string | null) => void;
}

/** Owns the QuickBooks Online credential form state, the Intuit Connect
 *  OAuth flow (including redirect-mode stash/restore), validation and
 *  scaffold-config shaping for the New Pipeline wizard.
 *
 * The realm id + refresh token are captured by the OAuth Connect flow, so
 * they're not manual inputs. Secret-name vars default to sensible names. */
export function useQuickBooksCredentials({
  open,
  projectId,
  onRestore,
  onError
}: UseQuickBooksCredentialsArgs) {
  const [clientId, setClientId] = useState("");
  const [realmId, setRealmId] = useState(""); // captured on Connect
  const [clientSecret, setClientSecret] = useState("");
  const [clientSecretName, setClientSecretName] = useState("QB_CLIENT_SECRET");
  const [refreshTokenName, setRefreshTokenName] = useState("QB_REFRESH_TOKEN");
  const [baseUrl, setBaseUrl] = useState("");
  // True once the Intuit Connect flow has stored the refresh token + realm id.
  const [connected, setConnected] = useState(false);
  const { connect, connecting } = useQuickBooksConnect(projectId);

  const reset = () => {
    setClientId("");
    setRealmId("");
    setClientSecret("");
    setClientSecretName("QB_CLIENT_SECRET");
    setRefreshTokenName("QB_REFRESH_TOKEN");
    setBaseUrl("");
    setConnected(false);
  };

  /** Persist the in-progress form so a redirect-mode Connect can restore it. */
  const stashWizard = (destinationDb: string | null, name: string, description: string) => {
    sessionStorage.setItem(
      QB_WIZARD_STASH_KEY,
      JSON.stringify({
        destinationDb,
        name,
        description,
        qbClientId: clientId,
        qbClientSecretName: clientSecretName,
        qbRefreshTokenName: refreshTokenName,
        qbBaseUrl: baseUrl,
        qbRealmId: realmId
      })
    );
  };

  // Restore the form after a full-page OAuth redirect (mobile / popup
  // blocked): the success page bounced back with `?qb_connected=ok&realm_id`
  // and the pre-redirect form was stashed in sessionStorage.
  // biome-ignore lint/correctness/useExhaustiveDependencies: onRestore is a fresh closure each render; this should only re-run when the dialog opens
  useEffect(() => {
    if (!open) return;
    const raw = sessionStorage.getItem(QB_WIZARD_STASH_KEY);
    if (!raw) return;
    sessionStorage.removeItem(QB_WIZARD_STASH_KEY);
    const params = new URLSearchParams(window.location.search);
    const isConnected = params.get("qb_connected") === "ok";
    try {
      const s = JSON.parse(raw) as Record<string, string>;
      setClientId(s.qbClientId ?? "");
      setClientSecretName(s.qbClientSecretName ?? "");
      setRefreshTokenName(s.qbRefreshTokenName ?? "");
      setBaseUrl(s.qbBaseUrl ?? "");
      setRealmId(params.get("realm_id") || s.qbRealmId || "");
      setConnected(isConnected);
      onRestore({
        destinationDb: s.destinationDb || null,
        name: s.name ?? "",
        description: s.description ?? ""
      });
    } catch {
      // Malformed stash — ignore and start fresh.
    }
    // Strip the one-shot query params so a refresh doesn't re-trigger.
    if (params.has("qb_connected") || params.has("realm_id")) {
      params.delete("qb_connected");
      params.delete("realm_id");
      const qs = params.toString();
      window.history.replaceState({}, "", `${window.location.pathname}${qs ? `?${qs}` : ""}`);
    }
  }, [open]);

  const handleConnect = async (destinationDb: string | null, name: string, description: string) => {
    if (!clientId.trim()) {
      onError("QuickBooks: Client ID is required before connecting");
      return;
    }
    if (!clientSecretName.trim() || !refreshTokenName.trim()) {
      onError("QuickBooks: client secret name and refresh token name are required");
      return;
    }
    if (!clientSecret.trim()) {
      onError("QuickBooks: enter the client secret so we can complete the OAuth exchange");
      return;
    }
    onError(null);
    try {
      const result = await connect({
        clientId,
        clientSecret,
        clientSecretVar: clientSecretName,
        refreshTokenVar: refreshTokenName,
        returnPath: window.location.href,
        stash: () => stashWizard(destinationDb, name, description)
      });
      // Popup mode resolves here; redirect mode navigates away.
      if (result) {
        if (result.realmId) setRealmId(result.realmId);
        setConnected(true);
        toast.success("QuickBooks connected", {
          description: "Refresh token stored. You can finish creating the pipeline."
        });
      }
    } catch {
      // The hook already surfaced a toast.
    }
  };

  /** Returns an error message, or null when the form is valid. */
  const validate = (): string | null => {
    if (!clientId.trim()) return "QuickBooks: Client ID is required";
    if (!clientSecretName.trim() || !refreshTokenName.trim())
      return "QuickBooks: secret names for the client secret and refresh token are required";
    // Connect captures the realm id + refresh token via OAuth, so it's the
    // required first step (Intuit only mints refresh tokens via consent).
    if (!connected || !realmId.trim())
      return "QuickBooks: click “Connect with QuickBooks” to authorize before creating";
    return null;
  };

  const buildScaffoldConfig = (): QuickBooksScaffoldConfig => ({
    clientId: clientId.trim(),
    clientSecretVar: clientSecretName.trim(),
    refreshTokenVar: refreshTokenName.trim(),
    realmId: realmId.trim(),
    baseUrl: baseUrl.trim() || undefined
  });

  return {
    clientId,
    setClientId,
    realmId,
    clientSecret,
    setClientSecret,
    clientSecretName,
    setClientSecretName,
    refreshTokenName,
    setRefreshTokenName,
    baseUrl,
    setBaseUrl,
    connected,
    connecting,
    handleConnect,
    reset,
    validate,
    buildScaffoldConfig
  };
}
