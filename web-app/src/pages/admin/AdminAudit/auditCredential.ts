import {
  maskedToken,
  TOKEN_KIND_LABELS
} from "@/components/settings/SettingsDialog/sections/account/PersonalTokens/accessSummary";
import type { TokenKind } from "@/types/apiToken";
import type { AuditEvent } from "@/types/audit";

/** The API key or token an audit row names, in the words the table shows. */
export interface AuditCredential {
  /**
   * `acted`: the credential that performed the action (`actor_type` is `api_key`). `subject`: the
   * token a lifecycle event made in a session is about.
   */
  role: "acted" | "subject";
  id: string | null;
  /** `null` when the row carries no name for it. */
  name: string | null;
  /** "Sandbox agent", or the raw kind for one this build has no name for. */
  kind: string | null;
  /** `oxy_sbx_Ab3x…`: the non-secret prefix, never the token. */
  prefix: string | null;
}

const kindLabel = (kind: string | null | undefined): string | null =>
  kind ? (TOKEN_KIND_LABELS[kind as TokenKind] ?? kind) : null;

/**
 * What a row says about a token, or `null` for a row that names none: every row written before
 * the fields existed, and every action taken in a session on something that is not a token.
 */
export const auditCredential = (event: AuditEvent): AuditCredential | null => {
  if (!event.token_id && !event.token_name && !event.token_kind && !event.token_prefix) {
    return null;
  }
  const acted = event.actor_type === "api_key";
  return {
    role: acted ? "acted" : "subject",
    id: event.token_id ?? null,
    // A lifecycle event names its token as the target, not in `token_name`.
    name: event.token_name || (acted ? null : event.target_label) || null,
    kind: kindLabel(event.token_kind),
    prefix: event.token_prefix
      ? maskedToken({ display_prefix: event.token_prefix, last_four: "" })
      : null
  };
};

/** Whether a row has anything to open: a token, an address or a client. */
export const hasAuditDetail = (event: AuditEvent): boolean =>
  auditCredential(event) !== null || !!event.ip || !!event.user_agent;

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/**
 * The `?token_id=` a link carried, if it is one the server will take. The route parses it as a
 * UUID and answers 400 to anything else, so a mangled link reads as no filter.
 */
export const tokenIdParam = (raw: string | null): string | undefined =>
  raw && UUID.test(raw) ? raw.toLowerCase() : undefined;
