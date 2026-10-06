import type { AuditEvent } from "@/types/audit";

export const TOKEN_ID = "0b5c1d6e-7f80-4a91-b2c3-d4e5f6a7b8c9";

/** What oxyc sends when an agent drives it: `oxyc/<version> agent/<label>`. */
export const AGENT_CLIENT = "oxyc/0.7.0 agent/live-check";

/** A row as the server sent it before the token and client fields existed. */
export const plainEvent = (over: Partial<AuditEvent> = {}): AuditEvent => ({
  id: "e-plain",
  created_at: new Date(Date.now() - 2 * 60 * 60 * 1000).toISOString(),
  actor_email: "ada@oxy.tech",
  actor_type: "user",
  action: "org.member.added",
  org_id: "11111111-2222-3333-4444-555555555555",
  workspace_id: null,
  partner_id: null,
  target_type: "user",
  target_id: "u-2",
  target_label: "lin@oxy.tech",
  outcome: "success",
  reason: null,
  via_global_override: false,
  ...over
});

/**
 * A row a sandbox agent token performed, as a running server sends it: `actor_type` is `api_key`
 * and `token_name` is set.
 */
export const tokenEvent = (over: Partial<AuditEvent> = {}): AuditEvent =>
  plainEvent({
    id: "e-token",
    actor_type: "api_key",
    action: "app.environment.created",
    target_type: "app",
    target_id: "a-1",
    target_label: "local/oxy-starter",
    token_id: TOKEN_ID,
    token_name: "live check",
    token_kind: "sandbox_agent",
    token_prefix: "oxy_sbx_Ab3x",
    ip: "203.0.113.7",
    user_agent: AGENT_CLIENT,
    ...over
  });

/**
 * A token lifecycle row, as a running server sends it: made as the person (`actor_type` is
 * `user`), `token_name` is `null`, and the token's name is the row's `target_label`. The kind and
 * the prefix are set.
 */
export const lifecycleEvent = (over: Partial<AuditEvent> = {}): AuditEvent =>
  plainEvent({
    id: "e-lifecycle",
    action: "token.created",
    target_type: "api_token",
    target_id: TOKEN_ID,
    target_label: "live check",
    token_id: TOKEN_ID,
    token_name: null,
    token_kind: "sandbox_agent",
    token_prefix: "oxy_sbx_Ab3x",
    ip: "198.51.100.4",
    user_agent: AGENT_CLIENT,
    ...over
  });
