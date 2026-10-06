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
