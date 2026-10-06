/** One audit event from `GET /admin/audit` (platform scope). */
export interface AuditEvent {
  id: string;
  created_at: string;
  actor_email: string;
  actor_type: string;
  action: string;
  org_id: string | null;
  workspace_id: string | null;
  partner_id: string | null;
  target_type: string | null;
  target_id: string | null;
  target_label: string | null;
  outcome: string;
  reason: string | null;
  /** The action was taken through the assume-role / global override. */
  via_global_override: boolean;
  /**
   * The API key or token the row names. When `actor_type` is `api_key` it is the credential that
   * performed the action. On a token lifecycle event made in a session (`token.created`, …) it is
   * the token the event is about. Absent from a server that predates it, `null` when the row
   * names none.
   */
  token_id?: string | null;
  /**
   * That token's name, on a row a key or token performed. A lifecycle event carries the name as
   * its `target_label` instead.
   */
  token_name?: string | null;
  /** `personal`, `legacy_key`, `service_account`, `ci` or `sandbox_agent`. */
  token_kind?: string | null;
  /** The non-secret display prefix, never the token. */
  token_prefix?: string | null;
  /** The address the load balancer saw. `null` for a row no request wrote. */
  ip?: string | null;
  /** As the client sent it: `oxyc/<version> agent/<label>` for an agent driving oxyc. */
  user_agent?: string | null;
}

/** Query params for the platform audit search. All optional. */
export interface AuditSearchParams {
  q?: string;
  action?: string;
  actor?: string;
  org_id?: string;
  outcome?: string;
  /** One API token: actions performed with it and its lifecycle events. */
  token_id?: string;
  limit?: number;
  offset?: number;
}
