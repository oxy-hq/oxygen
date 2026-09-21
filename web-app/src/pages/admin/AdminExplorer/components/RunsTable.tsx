import { ChevronDown, ChevronRight, ExternalLink, Play } from "lucide-react";
import { useState } from "react";
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
import { cn } from "@/libs/utils/cn";
import ROUTES from "@/libs/utils/routes";
import { ADMIN_TONE, type AdminTone } from "@/pages/admin/components/adminTone";
import type { ExplorerRun } from "@/services/api/adminExplorer";
import { ago, tenantLabel } from "../format";

/**
 * Cross-tenant run results — the agentic execution behind a thread. Rows
 * expand to the full error + originating question, and link out to the run's
 * thread inside its workspace. Status carries a colored dot; failed/dead runs
 * get a left accent so a wall of results reads as a heat strip.
 */
export const RunsTable = ({ rows }: { rows: ExplorerRun[] }) => {
  const [open, setOpen] = useState<Set<string>>(new Set());
  if (rows.length === 0) {
    return (
      <div className='flex flex-col items-center justify-center gap-2 rounded-lg border border-border/60 border-dashed bg-muted/20 px-6 py-12 text-center'>
        <Play className='size-6 text-muted-foreground' />
        <p className='font-medium text-xs'>No runs match.</p>
        <p className='text-muted-foreground text-xs'>Search by question, error, or run id.</p>
      </div>
    );
  }

  const toggle = (id: string) =>
    setOpen((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  return (
    <div className='overflow-hidden rounded-lg border border-border/60 bg-card'>
      <Table>
        <TableHeader>
          <TableRow className='hover:bg-transparent'>
            <TableHead className='w-8' aria-hidden />
            <TableHead className='text-[10px] uppercase tracking-[0.14em]'>
              Run · Question
            </TableHead>
            <TableHead className='text-[10px] uppercase tracking-[0.14em]'>
              Workspace · Org
            </TableHead>
            <TableHead className='text-[10px] uppercase tracking-[0.14em]'>Status</TableHead>
            <TableHead className='text-right text-[10px] uppercase tracking-[0.14em]'>
              Age
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.map((r) => (
            <RunRow key={r.id} run={r} expanded={open.has(r.id)} onToggle={() => toggle(r.id)} />
          ))}
        </TableBody>
      </Table>
    </div>
  );
};

const RunRow = ({
  run,
  expanded,
  onToggle
}: {
  run: ExplorerRun;
  expanded: boolean;
  onToggle: () => void;
}) => {
  const status = run.task_status ?? "unknown";
  const accent = statusAccent(status);
  const accentBorder = borderForStatus(status);
  const threadOpenable = run.org_slug && run.workspace_id && run.thread_id;
  return (
    <>
      <TableRow
        className='cursor-pointer text-xs'
        role='button'
        tabIndex={0}
        aria-expanded={expanded}
        data-testid={`admin-explorer-run-row-${run.id}`}
        onClick={onToggle}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            onToggle();
          }
        }}
      >
        <TableCell className={cn("border-l-2", accentBorder)}>
          {expanded ? (
            <ChevronDown className='size-3.5 text-muted-foreground' />
          ) : (
            <ChevronRight className='size-3.5 text-muted-foreground' />
          )}
        </TableCell>
        <TableCell className='max-w-0'>
          <div className='flex min-w-0 items-center gap-2'>
            {run.source_type ? (
              <span className='shrink-0 rounded-full bg-muted/60 px-1.5 py-0.5 font-mono text-[9px]'>
                {run.source_type}
              </span>
            ) : null}
            <span className='truncate'>{run.question_snippet || "(no question)"}</span>
          </div>
        </TableCell>
        <TableCell className='max-w-40 truncate text-muted-foreground'>
          {tenantLabel(run.workspace_name, run.org_name)}
        </TableCell>
        <TableCell>
          <span className='flex items-center gap-1.5'>
            <span className={cn("size-1.5 rounded-full", accent.dot)} aria-hidden />
            <span className={cn("font-medium text-[10px] uppercase tracking-wide", accent.text)}>
              {status}
            </span>
          </span>
        </TableCell>
        <TableCell className='text-right text-muted-foreground tabular-nums'>
          {ago(run.created_at)}
        </TableCell>
      </TableRow>

      {expanded ? (
        <TableRow className='hover:bg-transparent'>
          <TableCell colSpan={5} className={cn("border-l-2 bg-muted/20", accentBorder)}>
            <div className='space-y-3 py-1'>
              {run.error_message ? (
                <div className='space-y-1'>
                  <span className='font-medium text-[10px] text-muted-foreground uppercase tracking-[0.14em]'>
                    Error
                  </span>
                  <pre className='max-h-40 overflow-auto whitespace-pre-wrap break-words rounded-md border border-destructive/30 bg-destructive/5 p-2 font-mono text-[11px] text-destructive'>
                    {run.error_message}
                  </pre>
                </div>
              ) : null}
              <dl className='grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-xs'>
                <dt className='text-muted-foreground'>Run id</dt>
                <dd className='break-all font-mono text-[11px]'>{run.id}</dd>
                <dt className='text-muted-foreground'>User</dt>
                <dd>{run.user_email ?? "—"}</dd>
              </dl>
              {threadOpenable ? (
                <Button asChild size='sm' variant='outline' className='h-7 gap-1.5'>
                  <Link
                    to={ROUTES.ORG(run.org_slug as string)
                      .WORKSPACE(run.workspace_id as string)
                      .THREAD(run.thread_id as string)}
                  >
                    Open thread
                    <ExternalLink className='size-3.5' />
                  </Link>
                </Button>
              ) : null}
            </div>
          </TableCell>
        </TableRow>
      ) : null}
    </>
  );
};

/** Run status → one of the console's five tones, so a run reads the same way a
 *  job or a compile does. `null` keeps an unrecognised status in the plain
 *  foreground rather than borrowing a colour that would read as a verdict. */
function statusAccent(status: string): { dot: string; text: string } {
  const tone = runTone(status);
  if (!tone) return { dot: "bg-foreground/50", text: "text-foreground" };
  const v = ADMIN_TONE[tone];
  return { dot: v.dot, text: v.text };
}

function runTone(status: string): AdminTone | null {
  switch (status) {
    case "failed":
    case "dead":
      return "danger";
    case "running":
    case "delegating":
    case "awaiting_input":
      return "info";
    case "done":
      return "ok";
    case "cancelled":
      return "muted";
    default:
      return null;
  }
}

function borderForStatus(status: string): string {
  if (status === "failed" || status === "dead") return "border-l-destructive";
  if (status === "running" || status === "delegating") return "border-l-primary/60";
  return "border-l-transparent";
}
