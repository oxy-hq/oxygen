import { useEffect, useState } from "react";
import { toast } from "sonner";
import { useCreateSecret } from "@/hooks/api/secrets/useSecretMutations";
import useSecrets from "@/hooks/api/secrets/useSecrets";
import useSpApiMarketplaces from "@/hooks/api/spApi/useSpApiMarketplaces";
import {
  firstOfLastMonth,
  retargetSpApiSecretName,
  SP_API_SECRET_DEFAULTS,
  type SpApiPartnerType
} from "../../../scaffold";

export interface SpApiScaffoldConfig {
  clientId: string;
  clientSecretVar: string;
  refreshTokenVar: string;
  marketplaceId: string;
  defaultStart: string;
  partnerType: SpApiPartnerType;
}

/** `YYYY-MM-DD`, which is what `<input type="date">` produces and what the
 *  connector's `parse_default_start` accepts alongside RFC 3339. */
const ISO_DATE = /^\d{4}-\d{2}-\d{2}$/;

/** Owns the Amazon SP-API credential form state, validation, secret
 *  persistence and scaffold-config shaping for the New Pipeline wizard.
 *
 * No OAuth flow, unlike QuickBooks: an SP-API refresh token is issued when the
 * app is authorized in the account's own console — Seller Central or Vendor
 * Central — so the operator already holds one and only needs to name the
 * secret it lives in.
 */
export function useSpApiCredentials() {
  const [clientId, setClientId] = useState("");
  const [clientSecret, setClientSecret] = useState("");
  const [partnerType, setPartnerTypeState] = useState<SpApiPartnerType>("seller");
  const [clientSecretName, setClientSecretName] = useState(
    SP_API_SECRET_DEFAULTS.seller.clientSecret
  );
  const [refreshToken, setRefreshToken] = useState("");
  const [refreshTokenName, setRefreshTokenName] = useState(
    SP_API_SECRET_DEFAULTS.seller.refreshToken
  );

  /** Changing the account re-points the secret NAMES and CLEARS the pasted
   *  values, because both belong to the account rather than to the form.
   *
   *  The names first. Two accounts hold different refresh tokens, and a vendor
   *  pipeline left on `SP_API_REFRESH_TOKEN` passes validation with a blank
   *  value — blank means "reuse the secret already stored under this name",
   *  and the seller pipeline created one. It would then run with the seller's
   *  token and be refused on every vendor report, which reads as a credentials
   *  problem rather than as the wrong credential entirely. A name the operator
   *  typed is left alone: only one still equal to the other account's default
   *  is re-pointed.
   *
   *  The values matter for the same reason and were missed at first, which
   *  made this WORSE than the case it was written to prevent. Paste the seller
   *  token, then realise this is the vendor pipeline and flip the select: the
   *  name became `SP_API_VENDOR_REFRESH_TOKEN`, the value stayed the seller's,
   *  and `persistSecret` stored it under the vendor name with a "Vendor
   *  Central" description. The mismatch is then invisible in the secret list
   *  too — the one place someone would later go to untangle it.
   *
   *  Clearing them is safe rather than destructive: blank already means "reuse
   *  the secret stored under this name", and the name is now the new
   *  account's, so a blank cannot reach the other account's secret either. */
  const setPartnerType = (next: SpApiPartnerType) => {
    if (next === partnerType) return;
    setClientSecretName((n) => retargetSpApiSecretName(n, partnerType, next, "clientSecret"));
    setRefreshTokenName((n) => retargetSpApiSecretName(n, partnerType, next, "refreshToken"));
    setClientSecret("");
    setRefreshToken("");
    setPartnerTypeState(next);
  };
  // Server-owned, so the picker cannot drift from `build_sp_api`'s refusal.
  // Empty until it loads; `validate` treats that as "not chosen yet" rather
  // than letting a blank reach the YAML.
  const marketplaces = useSpApiMarketplaces();
  const [marketplaceId, setMarketplaceId] = useState<string>("");

  // Select the first once the list arrives, so the form is valid by default
  // rather than making every operator pick before they can continue. Only
  // when nothing is chosen yet — never clobbering a real choice, including
  // after a refetch.
  useEffect(() => {
    const first = marketplaces.data?.[0]?.id;
    if (first && !marketplaceId) setMarketplaceId(first);
  }, [marketplaces.data, marketplaceId]);
  const [defaultStart, setDefaultStart] = useState(firstOfLastMonth());
  // This form stores up to two secrets on one submit. The hook would say
  // "Secret created successfully" for each; `persistSecret` says it once.
  const createSecret = useCreateSecret({ toastOnSuccess: false });
  // Needed to tell "reuse the existing secret" from "reference one that was
  // never created" — a blank value means the former, and without this the two
  // are indistinguishable at the point the operator can still fix it.
  const secrets = useSecrets();

  const reset = () => {
    setClientId("");
    setClientSecret("");
    setPartnerTypeState("seller");
    setClientSecretName(SP_API_SECRET_DEFAULTS.seller.clientSecret);
    setRefreshToken("");
    setRefreshTokenName(SP_API_SECRET_DEFAULTS.seller.refreshToken);
    setMarketplaceId("");
    setDefaultStart(firstOfLastMonth());
  };

  /** Returns an error message, or null when the form is valid.
   *
   * These mirror `build_sp_api`'s own refusals. Catching them here turns a
   * failed pipeline run into a form error the operator can act on.
   */
  const validate = (): string | null => {
    if (!clientId.trim()) return "Amazon SP-API: LWA Client ID is required";
    if (!clientSecretName.trim())
      return "Amazon SP-API: a secret name for the client secret is required";
    if (!refreshTokenName.trim())
      return "Amazon SP-API: a secret name for the refresh token is required";
    // Checked against the SERVER's list, so this cannot disagree with the
    // refusal `build_sp_api` would raise at run time.
    if (!marketplaceId) return "Amazon SP-API: a marketplace is required";
    if (marketplaces.data && !marketplaces.data.some((m) => m.id === marketplaceId))
      return "Amazon SP-API: pick a North America marketplace — the connector only reaches that endpoint";
    // Required, not defaulted: this is the whole backfill policy for a
    // forward-only connector, and both a too-recent and a too-old value cost
    // something silently — missing history, or a first run that spends one
    // report job per chunk against a per-account budget. The backend refuses
    // without it rather than guessing.
    if (!defaultStart.trim()) return "Amazon SP-API: a start date is required";
    if (!ISO_DATE.test(defaultStart.trim())) return "Amazon SP-API: start date must be YYYY-MM-DD";

    // A blank value means "reuse the secret already stored under this name".
    // Nothing checked that one existed, so with every other field defaulted,
    // typing only the client id wrote a pipeline whose `*_var` names point at
    // secrets nobody created. The first symptom was a run failing with
    // "`client_secret` is empty or unset", which reads as "the secret is
    // wrong" rather than "you never made one".
    //
    // Checked against EACH OTHER before the server list, because the server
    // cannot see this one: two brand-new secrets sharing a name both pass the
    // "does not exist yet" test, then `persistSecret` runs two sequential
    // creates on that name and the second 409s — stranding the first, which
    // now holds the client secret under a name the operator will next read as
    // a refresh token. Same failure the server-side check closes, reached
    // through the form instead.
    if (clientSecretName.trim() === refreshTokenName.trim()) {
      return "Amazon SP-API: the client secret and the refresh token need different secret names";
    }

    // Only enforced once the list has loaded — a failed secrets query should
    // not block the wizard, it just costs this particular check.
    const existing = secrets.data?.secrets;
    if (existing) {
      const has = (name: string) => existing.some((s) => s.name === name);
      for (const [value, name, field] of [
        [clientSecret, clientSecretName, "client secret"],
        [refreshToken, refreshTokenName, "refresh token"]
      ] as const) {
        const trimmedName = name.trim();
        if (!value.trim() && !has(trimmedName)) {
          return `Amazon SP-API: no secret named ${trimmedName} exists — paste the ${field} so it can be created, or point at an existing secret`;
        }
        // Creating over an existing name 409s. Worse, the two creates run in
        // sequence, so a collision on the second strands the first and every
        // retry hits the same 409.
        if (value.trim() && has(trimmedName)) {
          return `Amazon SP-API: a secret named ${trimmedName} already exists — clear the ${field} to reuse it, or choose another name`;
        }
      }
    }
    return null;
  };

  const buildScaffoldConfig = (): SpApiScaffoldConfig => ({
    clientId: clientId.trim(),
    clientSecretVar: clientSecretName.trim(),
    refreshTokenVar: refreshTokenName.trim(),
    marketplaceId,
    defaultStart: defaultStart.trim(),
    partnerType
  });

  /** Stores both credentials in the secret manager. A blank value means
   *  "reuse an existing secret with this name" — skip create, same as Toast. */
  const persistSecret = async (pipelineName: string) => {
    // The account is named in the description because the two tokens are
    // indistinguishable once stored — both are `Atzr|…` — and the secret list
    // is where someone later works out which pipeline a name belongs to.
    const account = partnerType === "vendor" ? "Vendor Central" : "Seller Central";
    const created: string[] = [];
    try {
      if (clientSecret.trim()) {
        await createSecret.mutateAsync({
          name: clientSecretName.trim(),
          value: clientSecret,
          description: `Amazon LWA client secret (${account}) for pipeline ${pipelineName}`
        });
        created.push(clientSecretName.trim());
      }
      if (refreshToken.trim()) {
        await createSecret.mutateAsync({
          name: refreshTokenName.trim(),
          value: refreshToken,
          description: `Amazon SP-API refresh token (${account}) for pipeline ${pipelineName}`
        });
        created.push(refreshTokenName.trim());
      }
    } finally {
      // One toast for the submit, naming what was stored. In a `finally` because a
      // second create that fails leaves the first one stored: the error toast alone
      // would hide the secret the next attempt then collides with.
      if (created.length === 1) toast.success(`Secret ${created[0]} created successfully`);
      if (created.length === 2) {
        toast.success(`Secrets ${created[0]} and ${created[1]} created successfully`);
      }
    }
  };

  return {
    clientId,
    setClientId,
    partnerType,
    setPartnerType,
    clientSecret,
    setClientSecret,
    clientSecretName,
    setClientSecretName,
    refreshToken,
    setRefreshToken,
    refreshTokenName,
    setRefreshTokenName,
    marketplaceId,
    setMarketplaceId,
    marketplaces,
    defaultStart,
    setDefaultStart,
    reset,
    validate,
    buildScaffoldConfig,
    persistSecret
  };
}
