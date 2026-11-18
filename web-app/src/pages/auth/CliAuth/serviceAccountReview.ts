import type { CliServiceAccountMintRequest, CliServiceAccountRole } from "@/types/apiToken";
import type { ServiceAccountAsk } from "./cliAuthRequest";

/**
 * oxyc's request for an organization's service-account token, as the `mint` the server is
 * asked about and then asked to approve — or why it can't be sent as it stands.
 *
 * Only what can't be sent at all is caught here: a role that is not member or admin, a lifetime
 * that is no whole number. Everything else — whether the organization exists and the person may
 * manage its service accounts, whether the account exists and at which role, the lifetime cap —
 * is the server's to say, and the page shows its answer (`useServiceAccountPreview`).
 */
export type ServiceAccountMint =
  | { mint: CliServiceAccountMintRequest }
  | { problem: string; command: string };

const ROLES: CliServiceAccountRole[] = ["member", "admin"];

/** The command that opened this request, as the person would have typed it. */
export const serviceAccountCommand = (ask: ServiceAccountAsk, deviceCode = false): string =>
  [
    `oxyc tokens create --org-service-account ${ask.org}/${ask.account}`,
    ...(ask.role && ask.role !== "member" ? [`--role ${ask.role}`] : []),
    ...(deviceCode ? ["--no-browser"] : [])
  ].join(" ");

export const serviceAccountMint = (ask: ServiceAccountAsk): ServiceAccountMint => {
  const command = serviceAccountCommand(ask);
  const role = (ask.role ?? "member") as CliServiceAccountRole;
  if (!ROLES.includes(role)) {
    return {
      problem: `A service account is a member or an admin, never ${JSON.stringify(ask.role)}.`,
      command
    };
  }
  let days: number | undefined;
  if (ask.days !== null) {
    const text = ask.days.trim();
    if (!/^\d+$/.test(text)) {
      return { problem: `The lifetime "${ask.days}" is not a whole number of days.`, command };
    }
    days = Number(text);
  }
  return {
    mint: {
      kind: "service_account",
      org: ask.org,
      account: ask.account,
      role,
      ...(ask.name !== null ? { name: ask.name } : {}),
      ...(days !== undefined ? { expires_in_days: days } : {})
    }
  };
};
