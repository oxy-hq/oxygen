import { isAxiosError } from "axios";
import { apiErrorCode, apiErrorMessage, apiStatus } from "@/libs/apiError";
import { exceedsPolicyMessage } from "@/libs/tokenPolicy";
import type { SandboxAgentLimits, SandboxApp } from "@/types/apiToken";

/** What the person was doing when the request failed. */
export type TokenAction = "create" | "update" | "rename" | "regenerate" | "revoke";

/** The `code` a token route put in its `{ error, code? }` body, if any. */
export const tokenErrorCode = apiErrorCode;

const FALLBACK: Record<TokenAction, string> = {
  create: "Couldn't create the token",
  update: "Couldn't update the token's access",
  rename: "Couldn't rename the token",
  regenerate: "Couldn't regenerate the token",
  revoke: "Couldn't revoke the token"
};

/** A grant named an org or workspace the caller can't reach; the route answers 404, not 403. */
const UNREACHABLE =
  "One of the selected organizations or workspaces is no longer available to you. Reopen the dialog and pick again.";

/**
 * 409 `sandbox_token_fixed`: a sandbox agent token is never renamed, re-scoped, extended or
 * regenerated. No row offers any of those for one, so this is what a stale page hears.
 */
const SANDBOX_TOKEN_FIXED =
  "A sandbox agent token can't be changed once it's created. Revoke it and create a new one instead.";

/**
 * 409 `agent_token_fixed`: an agent token is never renamed, re-scoped, extended or regenerated
 * either. It can be revoked, and the agent asks for a new one (`oxyc tokens create --agent`).
 */
const AGENT_TOKEN_FIXED =
  "An agent token can't be changed once it's approved. Revoke it, and have the agent ask for a new one.";

/** 403 `session_required` where a token is being minted: only a signed-in browser may. */
const MINT_NEEDS_SESSION = "Creating a token needs a browser session. Sign in again, then retry.";

/**
 * Toast copy for a failed `/user/tokens` request. The contract's codes come first, since two of
 * them share a status (403 `standing_required` / `session_required`).
 *
 * `legacy_immutable` is not handled: these routes serve personal access tokens only, and a legacy
 * API key's id answers 404 here, like any id that is not a token.
 */
export const tokenErrorMessage = (error: unknown, action: TokenAction): string => {
  const capped = exceedsPolicyMessage(error, action === "regenerate" ? "regenerate" : "set_expiry");
  if (capped) return capped;
  switch (tokenErrorCode(error)) {
    case "standing_required":
      return "Your account doesn't hold staff or partner access, so a token can't include it. Clear those options and try again.";
    case "revoked":
      return "This token was revoked, so it can't be changed.";
    case "session_required":
      return "Managing tokens needs a browser session. Sign in again, then retry.";
    case "sandbox_token_fixed":
      return SANDBOX_TOKEN_FIXED;
    case "agent_token_fixed":
      return AGENT_TOKEN_FIXED;
    case undefined:
      // No contract code: the status decides, below.
      break;
  }
  switch (apiStatus(error)) {
    case 400:
      return apiErrorMessage(error, FALLBACK[action]);
    case 404:
      return action === "create" || action === "update"
        ? UNREACHABLE
        : "This token no longer exists";
    default:
      return FALLBACK[action];
  }
};

/** The `app_id` a 404 `app_not_found` names: the one app the mint was refused for. */
const refusedAppId = (error: unknown): string | undefined => {
  if (!isAxiosError(error)) return undefined;
  const data: unknown = error.response?.data;
  if (!data || typeof data !== "object") return undefined;
  const appId = (data as { app_id?: unknown }).app_id;
  return typeof appId === "string" && appId ? appId : undefined;
};

/**
 * Where a mint was asked from. The refusal is the same; what the person can do about it is not:
 * in the dialog they pick again, on the `/cli-auth` approval the request is oxyc's to change.
 */
export type MintSurface = "dialog" | "cli";

const MINT_ADVICE: Record<MintSurface, { withoutApp: string; failed: string }> = {
  dialog: {
    withoutApp: "Pick other apps and try again.",
    failed: "Couldn't create the sandbox agent token. Try again."
  },
  cli: {
    withoutApp: "Run oxyc again without it.",
    failed: "Couldn't approve the request. Try again, or run the oxyc command again for a new link."
  }
};

/**
 * Copy for a refused sandbox agent mint, whether the create dialog asked (`POST /user/tokens`)
 * or the `/cli-auth` approval did (`POST /auth/cli/authorize`). Both run the same checks.
 *
 * `apps` is what the person was choosing from, to name the app a 404 refuses. The route answers
 * 404 alike for an app that is gone and one the caller may no longer build, so the sentence
 * claims neither.
 *
 * Every refusal carries the server's sentence as `error`. `invalid_sandbox_token` repeats it as
 * `message`; `exceeds_policy` adds `max_lifetime_days` and has no `message`.
 */
export const sandboxMintErrorMessage = (
  error: unknown,
  apps: readonly SandboxApp[],
  limits: SandboxAgentLimits,
  surface: MintSurface = "dialog"
): string => {
  const advice = MINT_ADVICE[surface];
  switch (tokenErrorCode(error)) {
    case "app_not_found": {
      const app = apps.find((candidate) => candidate.id === refusedAppId(error));
      return app
        ? `Oxygen couldn't find ${app.name} in ${app.org_name} for you. It may be gone, or you may no longer build apps for that organization. ${advice.withoutApp}`
        : `One of the apps is no longer available to you. ${advice.withoutApp}`;
    }
    case "invalid_sandbox_token":
      return `Oxygen refused that request. A sandbox agent token names 1 to ${limits.max_apps} apps and lasts 1 to ${limits.max_hours} hours.`;
    case "exceeds_policy":
      // An organization's cap on how long a token may last. The server's sentence names the cap,
      // in days, where the lifetime asked for is in hours: it is shown as it is.
      return apiErrorMessage(error, advice.failed);
    case "session_required":
      return MINT_NEEDS_SESSION;
    case undefined:
      break;
  }
  return apiStatus(error) === 400 ? apiErrorMessage(error, FALLBACK.create) : advice.failed;
};

/**
 * Copy for a refused agent token approval on `/cli-auth` (`POST /auth/cli/authorize` with
 * `mint.kind: "agent"`). There is nothing to pick again on that page: the request is oxyc's.
 *
 * A 400 carries the server's sentence as `error`, and it is shown as it is: `invalid_agent_token`
 * says which part of the request it refused, and `exceeds_policy` names an organization's cap on
 * a token's lifetime. `session_required` never comes from a browser that signed in; it is a
 * refusal like any other, and the page stays where it is.
 */
export const agentMintErrorMessage = (error: unknown): string => {
  const failed = MINT_ADVICE.cli.failed;
  if (tokenErrorCode(error) === "session_required") return MINT_NEEDS_SESSION;
  return apiStatus(error) === 400 ? apiErrorMessage(error, failed) : failed;
};

/** A 404 `app_not_found`: the apps on offer are out of date and worth reading again. */
export const isUnavailableAppError = (error: unknown): boolean =>
  tokenErrorCode(error) === "app_not_found";

/** The row on screen no longer matches the server: it was revoked or deleted elsewhere. */
export const isStaleTokenError = (error: unknown): boolean =>
  tokenErrorCode(error) === "revoked" || apiStatus(error) === 404;
