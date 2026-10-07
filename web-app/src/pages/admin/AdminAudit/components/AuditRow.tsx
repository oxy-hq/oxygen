import { ChevronRight, ShieldAlert } from "lucide-react";
import { useId, useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { TableCell, TableRow } from "@/components/ui/shadcn/table";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { AuditEvent } from "@/types/audit";
import { auditCredential, hasAuditDetail } from "../auditCredential";
import { AuditDetail } from "./AuditDetail";

/** The toggle, then When, Actor, Action, Target, Scope and Outcome. */
const AUDIT_COLUMNS = 7;

/** Compact "2h" / "3d"; full timestamp on hover. */
function ago(iso: string): string {
  const s = Math.max(0, (Date.now() - new Date(iso).getTime()) / 1000);
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  if (s < 2592000) return `${Math.floor(s / 86400)}d`;
  return new Date(iso).toLocaleDateString();
}

/** Short scope label; full id on hover. */
function scopeLabel(e: AuditEvent): string {
  if (e.org_id) return `org·${e.org_id.slice(0, 6)}`;
  if (e.partner_id) return `ptnr·${e.partner_id.slice(0, 6)}`;
  return "platform";
}

/** Tint the action by category so the eye groups them without reading each. */
function actionTone(action: string): string {
  if (/\.(revoked|removed|deleted|deactivated|denied)$/.test(action)) return "text-destructive/80";
  if (action.startsWith("partner.")) return "text-primary";
  return "text-foreground";
}

interface Props {
  event: AuditEvent;
  /** Narrow the log to one token. Absent when the log is already narrowed to this row's. */
  onFilterToken?: (tokenId: string) => void;
}

/**
 * One event. A row a key or token performed says so under its actor, "via" the token's name and
 * kind, with the prefix on hover. The token, the address and the client are in the detail the
 * chevron opens, so no column is added for them.
 *
 * A row that names no token, address or client has no "via" line and no chevron: its cells are
 * the ones it always had.
 */
export function AuditRow({ event: e, onFilterToken }: Props) {
  const [open, setOpen] = useState(false);
  const detailId = useId();
  const credential = auditCredential(e);
  const expandable = hasAuditDetail(e);
  const failed = e.outcome !== "success";

  return (
    <>
      <TableRow
        className={cn("border-border/50", failed && "bg-destructive/5")}
        title={e.reason ?? undefined}
        data-testid={`admin-audit-row-${e.id}`}
      >
        <TableCell className='w-6 py-1 pr-0'>
          {expandable && (
            <Button
              variant='ghost'
              size='icon'
              className='size-5 text-muted-foreground'
              onClick={() => setOpen((was) => !was)}
              aria-expanded={open}
              aria-controls={open ? detailId : undefined}
              aria-label={
                open ? "Hide the credential and client" : "Show the credential and client"
              }
              data-testid='admin-audit-toggle'
            >
              <ChevronRight className={cn("size-3 transition-transform", open && "rotate-90")} />
            </Button>
          )}
        </TableCell>
        <TableCell
          className='whitespace-nowrap py-1 text-muted-foreground tabular-nums'
          title={new Date(e.created_at).toLocaleString()}
        >
          {ago(e.created_at)}
        </TableCell>
        <TableCell className='py-1'>
          <span className='truncate'>{e.actor_email}</span>
          {e.actor_type !== "user" && (
            <span className='ml-1 text-[10px] text-muted-foreground'>({e.actor_type})</span>
          )}
          {credential?.role === "acted" && (
            <div
              className='text-[10px] text-muted-foreground leading-tight'
              title={credential.prefix ?? undefined}
              data-testid='admin-audit-token'
            >
              via{" "}
              <span className='text-foreground/80'>
                {credential.name ?? "a token with no name"}
              </span>
              {credential.kind && <span> · {credential.kind}</span>}
            </div>
          )}
        </TableCell>
        <TableCell className='py-1'>
          <div className='flex items-center gap-1.5'>
            <span className={cn("font-mono", actionTone(e.action))}>{e.action}</span>
            {e.via_global_override && (
              <span
                className={cn(
                  "inline-flex items-center gap-0.5 rounded-sm px-1 font-medium text-[10px]",
                  ADMIN_TONE.warn.bg,
                  ADMIN_TONE.warn.text
                )}
                title='Taken through the assume-role / global override'
              >
                <ShieldAlert className='size-3' />
                override
              </span>
            )}
          </div>
        </TableCell>
        <TableCell
          className='max-w-48 truncate py-1 text-muted-foreground'
          title={e.target_id ?? undefined}
        >
          {e.target_label || e.target_type || "—"}
        </TableCell>
        <TableCell
          className='py-1 font-mono text-muted-foreground'
          title={e.org_id ?? e.partner_id ?? "platform"}
        >
          {scopeLabel(e)}
        </TableCell>
        <TableCell className='py-1 text-right'>
          {failed ? (
            <span className='font-medium text-destructive'>failed</span>
          ) : (
            <span
              className='inline-block size-1.5 rounded-full bg-muted-foreground/40 align-middle'
              title='success'
            />
          )}
        </TableCell>
      </TableRow>
      {open && (
        <AuditDetail
          id={detailId}
          event={e}
          credential={credential}
          columns={AUDIT_COLUMNS}
          onFilterToken={onFilterToken}
        />
      )}
    </>
  );
}
