import { ExternalLink, MessageSquare } from "lucide-react";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from "@/components/ui/shadcn/table";
import ROUTES from "@/libs/utils/routes";
import type { ExplorerThread } from "@/services/api/adminExplorer";
import { ago, tenantLabel } from "../format";

/**
 * Cross-tenant thread results. Each row drills straight into the conversation
 * inside its workspace (operators can now open non-member workspaces). The
 * input snippet is the "what was this about?" at-a-glance read.
 */
export const ThreadsTable = ({ rows }: { rows: ExplorerThread[] }) => {
  if (rows.length === 0) {
    return <Empty />;
  }
  return (
    <div className='overflow-hidden border border-border/60 bg-card'>
      <Table>
        <TableHeader>
          <TableRow className='hover:bg-transparent'>
            <TableHead className='text-[10px] uppercase tracking-[0.14em]'>Thread</TableHead>
            <TableHead className='text-[10px] uppercase tracking-[0.14em]'>
              Workspace · Org
            </TableHead>
            <TableHead className='text-[10px] uppercase tracking-[0.14em]'>User</TableHead>
            <TableHead className='text-right text-[10px] uppercase tracking-[0.14em]'>
              Age
            </TableHead>
            <TableHead className='w-10' aria-hidden />
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.map((t) => {
            const openable = t.org_slug && t.workspace_id;
            return (
              <TableRow key={t.id} className='text-xs'>
                <TableCell className='max-w-0'>
                  <div className='flex items-center gap-2'>
                    <MessageSquare className='size-3.5 shrink-0 text-muted-foreground' />
                    <span className='truncate font-medium'>{t.title || "(untitled)"}</span>
                    {t.is_processing ? (
                      <span className='shrink-0 rounded-full bg-primary/10 px-1.5 py-0.5 font-medium text-[9px] text-primary uppercase'>
                        live
                      </span>
                    ) : null}
                  </div>
                  {t.input_snippet ? (
                    <p className='truncate pl-5 text-[11px] text-muted-foreground'>
                      {t.input_snippet}
                    </p>
                  ) : null}
                </TableCell>
                <TableCell className='max-w-40 truncate text-muted-foreground'>
                  {tenantLabel(t.workspace_name, t.org_name)}
                </TableCell>
                <TableCell className='max-w-40 truncate font-mono text-[11px] text-muted-foreground'>
                  {t.user_email ?? "—"}
                </TableCell>
                <TableCell className='text-right text-muted-foreground tabular-nums'>
                  {ago(t.created_at)}
                </TableCell>
                <TableCell>
                  {openable ? (
                    <Button
                      asChild
                      size='sm'
                      variant='ghost'
                      className='h-6 w-6 p-0 text-muted-foreground hover:text-foreground'
                    >
                      <Link
                        to={ROUTES.ORG(t.org_slug as string)
                          .ENTER_WORKSPACE(t.workspace_id as string)
                          .THREAD(t.id)}
                        title='Open thread'
                      >
                        <ExternalLink className='size-3.5' />
                      </Link>
                    </Button>
                  ) : null}
                </TableCell>
              </TableRow>
            );
          })}
        </TableBody>
      </Table>
    </div>
  );
};

const Empty = () => (
  <div className='flex flex-col items-center justify-center gap-2 border border-border/60 border-dashed bg-muted/20 px-6 py-12 text-center'>
    <MessageSquare className='size-6 text-muted-foreground' />
    <p className='font-medium text-xs'>No threads match.</p>
    <p className='text-muted-foreground text-xs'>Search by title, content, or thread id.</p>
  </div>
);
