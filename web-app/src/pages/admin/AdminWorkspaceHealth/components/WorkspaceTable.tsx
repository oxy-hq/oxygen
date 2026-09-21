import { Link } from "react-router-dom";
import { Table, TableBody, TableCell, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import { timeAgo } from "@/libs/utils/date";
import ROUTES from "@/libs/utils/routes";
import { AdminStatusPill } from "@/pages/admin/components/AdminStatusPill";
import {
  ADMIN_HEADER_ROW_CLASS,
  ADMIN_ROW_CLASS,
  AdminTh
} from "@/pages/admin/components/AdminTable";
import { workspaceHealthTone } from "@/pages/admin/components/workspaceHealthTone";
import type { WorkspaceHealthEntry } from "@/services/api/workspaceHealth";

/**
 * The original per-workspace rollup, kept because "is *this* workspace ok?" is a real
 * question the cause view answers only indirectly — and because an operator who has the
 * old orientation in their fingers should not have to relearn the page to get an answer
 * they used to get.
 *
 * The one change: the reason wraps (`whitespace-pre-wrap break-words`) instead of running
 * off the right edge of the viewport.
 */
export const WorkspaceTable = ({ workspaces }: { workspaces: WorkspaceHealthEntry[] }) => (
  <div className='overflow-hidden rounded-lg border border-border/60'>
    <Table>
      <TableHeader>
        <TableRow className={ADMIN_HEADER_ROW_CLASS}>
          <AdminTh>Workspace</AdminTh>
          <AdminTh>Status</AdminTh>
          <AdminTh>Reasons</AdminTh>
          <AdminTh align='right'>Last checked</AdminTh>
        </TableRow>
      </TableHeader>
      <TableBody>
        {workspaces.map((ws) => (
          <TableRow
            key={ws.workspace_id}
            className={ADMIN_ROW_CLASS}
            data-testid='workspace-health-row'
            data-status={ws.status}
          >
            <TableCell className='align-top'>
              <Link
                to={`${ROUTES.ADMIN.WORKSPACE_DETAIL(ws.workspace_id)}?tab=health`}
                className='group block'
              >
                <span className='font-medium text-xs group-hover:underline'>
                  {ws.workspace_name ?? "Unknown workspace"}
                </span>
                <span className='block text-muted-foreground text-xs'>
                  {ws.org_name ? `${ws.org_name} · ` : ""}
                  <span className='font-mono text-[10px]'>{ws.workspace_id}</span>
                </span>
              </Link>
            </TableCell>
            <TableCell className='align-top'>
              <AdminStatusPill
                tone={workspaceHealthTone(ws.status)}
                label={ws.status}
                data-testid='workspace-health-status-badge'
              />
            </TableCell>
            <TableCell className='align-top'>
              {ws.reasons.length === 0 ? (
                <span className='text-muted-foreground/60'>—</span>
              ) : (
                <ul className='list-none space-y-1'>
                  {ws.reasons.map((reason) => (
                    <li
                      key={reason}
                      className='whitespace-pre-wrap break-words font-mono text-muted-foreground text-xs leading-relaxed'
                    >
                      {reason}
                    </li>
                  ))}
                </ul>
              )}
            </TableCell>
            <TableCell className='text-right align-top text-muted-foreground text-xs tabular-nums'>
              {ws.checked_at ? (
                <span title={new Date(ws.checked_at).toLocaleString()}>
                  {timeAgo(ws.checked_at)}
                </span>
              ) : (
                <span className='text-muted-foreground/50'>—</span>
              )}
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  </div>
);
