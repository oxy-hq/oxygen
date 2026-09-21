import { AlertTriangle, ArrowRight, Building2, FolderOpen, Users } from "lucide-react";
import { Link } from "react-router-dom";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import { AdminEmptyState } from "../../components/AdminEmptyState";
import { AdminSectionLabel } from "../../components/AdminSectionLabel";

export type AttentionRow = {
  icon: typeof AlertTriangle;
  /** Number that prefixes the row, in tabular-nums. */
  count: number;
  label: string;
  to: string;
  /** When set, the row gets a `warn` tint instead of the default subtle. */
  severity?: "warn" | "danger";
};

/**
 * Operator triage feed. Surfaces the small set of cross-tenant anomalies
 * the overview hub can derive from list data (no extra endpoints):
 *   - orgs that look abandoned
 *   - users without any org
 *   - workspaces that failed to clone or have no org
 * Each row links into the resource list where the operator can act.
 *
 * The component takes derived counts + label tuples so the overview hub
 * stays the source of truth for the predicates.
 */
export const NeedsAttention = ({
  rows,
  isLoading
}: {
  rows: AttentionRow[];
  isLoading?: boolean;
}) => {
  const visible = rows.filter((r) => r.count > 0);
  return (
    <div className='space-y-3'>
      <AdminSectionLabel
        trailing={
          isLoading
            ? "…"
            : visible.length
              ? `${visible.length} signal${visible.length === 1 ? "" : "s"}`
              : undefined
        }
      >
        Needs attention
      </AdminSectionLabel>
      {isLoading ? (
        <div className='space-y-1.5'>
          {[0, 1, 2].map((i) => (
            <div key={i} className='h-11 animate-pulse rounded-md bg-muted/40' />
          ))}
        </div>
      ) : visible.length === 0 ? (
        <AdminEmptyState
          icon={AlertTriangle}
          title='Quiet across the fleet'
          description='No abandoned orgs, orphan users, or failed workspaces right now. Check back after the next sync.'
        />
      ) : (
        <ul className='divide-y divide-border/60 overflow-hidden rounded-md border border-border/60 bg-card'>
          {visible.map((row) => (
            <li key={row.label}>
              <Link
                to={row.to}
                className='group flex items-center gap-3 px-3 py-2.5 transition-colors hover:bg-muted/40'
              >
                <row.icon
                  className={cn(
                    "size-4 shrink-0",
                    row.severity === "danger"
                      ? ADMIN_TONE.danger.text
                      : row.severity === "warn"
                        ? ADMIN_TONE.warn.text
                        : ADMIN_TONE.muted.text
                  )}
                />
                <span className='font-semibold text-xs tabular-nums'>{row.count}</span>
                <span className='text-muted-foreground text-xs'>{row.label}</span>
                <ArrowRight className='ml-auto size-3.5 text-muted-foreground/60 transition-colors group-hover:text-foreground' />
              </Link>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
};

/** Re-exported icons so the page can compose rows without importing twice. */
export const ATTENTION_ICONS = {
  org: Building2,
  user: Users,
  workspace: FolderOpen,
  warn: AlertTriangle
} as const;

/** Route helpers so attention rows can deep-link with the right filter. */
export const attentionLink = {
  staleOrgs: ROUTES.ADMIN.ORGS,
  orglessUsers: ROUTES.ADMIN.USERS,
  failedWorkspaces: ROUTES.ADMIN.WORKSPACES,
  orphanWorkspaces: ROUTES.ADMIN.WORKSPACES
} as const;
