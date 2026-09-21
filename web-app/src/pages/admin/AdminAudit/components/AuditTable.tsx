import { ShieldAlert } from "lucide-react";
import { Table, TableBody, TableCell, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_HEADER_ROW_CLASS, AdminTh } from "@/pages/admin/components/AdminTable";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { AuditEvent } from "@/types/audit";

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

/**
 * The loaded stream. Loading, failure and "nothing matched" are no longer this
 * component's business — the page gates all three through `AdminAsync`, which
 * is also where the retry the old inline error block never offered now lives.
 * So `events` arrives already resolved and non-empty.
 */
export default function AuditTable({ events, limit }: { events: AuditEvent[]; limit: number }) {
  return (
    <div className='space-y-2'>
      <div className='overflow-x-auto rounded-md border border-border/60'>
        <Table className='text-xs'>
          <TableHeader>
            <TableRow className={ADMIN_HEADER_ROW_CLASS}>
              <AdminTh>When</AdminTh>
              <AdminTh>Actor</AdminTh>
              <AdminTh>Action</AdminTh>
              <AdminTh>Target</AdminTh>
              <AdminTh>Scope</AdminTh>
              <AdminTh align='right'>Outcome</AdminTh>
            </TableRow>
          </TableHeader>
          <TableBody>
            {events.map((e) => {
              const failed = e.outcome !== "success";
              return (
                <TableRow
                  key={e.id}
                  className={cn("border-border/50", failed && "bg-destructive/5")}
                  title={e.reason ?? undefined}
                  data-testid={`admin-audit-row-${e.id}`}
                >
                  <TableCell
                    className='whitespace-nowrap py-1 text-muted-foreground tabular-nums'
                    title={new Date(e.created_at).toLocaleString()}
                  >
                    {ago(e.created_at)}
                  </TableCell>
                  <TableCell className='py-1'>
                    <span className='truncate'>{e.actor_email}</span>
                    {e.actor_type !== "user" && (
                      <span className='ml-1 text-[10px] text-muted-foreground'>
                        ({e.actor_type})
                      </span>
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
              );
            })}
          </TableBody>
        </Table>
      </div>
      {events.length >= limit && (
        <p className='text-muted-foreground text-xs'>
          Showing the most recent {limit}. Narrow the filters to reach older events.
        </p>
      )}
    </div>
  );
}
