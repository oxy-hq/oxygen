import { AlertTriangle, MoreHorizontal, Pencil, Play, Trash2 } from "lucide-react";
import type React from "react";
import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Badge } from "@/components/ui/shadcn/badge";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger
} from "@/components/ui/shadcn/dropdown-menu";
import { Switch } from "@/components/ui/shadcn/switch";
import { TableCell, TableRow } from "@/components/ui/shadcn/table";
import { useDeleteSchedule, useUpdateSchedule } from "@/hooks/api/schedules/useSchedules";
import { cn } from "@/libs/shadcn/utils";
import type { Schedule, ScheduleInput } from "@/types/schedule";
import { isSystemSchedule, targetKindToJobType } from "../../components/constants";
import { JobTypeBadge } from "../../components/JobTypeBadge";
import { StatusBadge } from "../../components/StatusBadge";
import { SystemBadge } from "../../components/SystemBadge";
import { useCoordinatorRoutes } from "../../components/useCoordinatorRoutes";
import { describeCron, formatRelative, formatTimestamp } from "../../components/utils";
import DeleteJobDialog from "./DeleteJobDialog";
import RunNowDialog from "./RunNowDialog";
import ScheduleDialog from "./ScheduleDialog";

const toInput = (s: Schedule): ScheduleInput => ({
  name: s.name,
  target_kind: s.target_kind,
  target_ref: s.target_ref,
  // Both carried for the same reason: the PATCH is a whole-row write, so a
  // field this helper omits is CLEARED. `question` is load-bearing for agent
  // schedules — `validate_input` rejects an empty one — so dropping it makes
  // the Enabled toggle 400 on any agent job.
  question: s.question,
  variables: s.variables,
  cron_expr: s.cron_expr,
  timezone: s.timezone,
  enabled: s.enabled
});

/** One job in the catalog — definition state plus inline operating actions. */
export const JobRow: React.FC<{ schedule: Schedule; canManage: boolean }> = ({
  schedule,
  canManage
}) => {
  const navigate = useNavigate();
  const routes = useCoordinatorRoutes();
  const update = useUpdateSchedule();
  const del = useDeleteSchedule();
  const [editOpen, setEditOpen] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [runOpen, setRunOpen] = useState(false);

  const jobType = targetKindToJobType(schedule.target_kind);
  const isSystem = isSystemSchedule(schedule);
  const lastStatus = schedule.last_error ? "failed" : schedule.last_fired_at ? "done" : null;

  const toggleEnabled = (enabled: boolean) =>
    update.mutate({ id: schedule.id, input: { ...toInput(schedule), enabled } });

  const stop = (e: React.MouseEvent) => e.stopPropagation();

  return (
    <>
      <TableRow
        data-testid='coordinator-job-row'
        data-schedule-id={schedule.id}
        className='cursor-pointer'
        onClick={() => navigate(routes.JOB_DETAIL(schedule.id))}
      >
        {/* Job — type, name, health */}
        <TableCell data-label='Job'>
          <div className='flex items-center gap-2'>
            {isSystem ? (
              <SystemBadge variant='icon' />
            ) : (
              <JobTypeBadge type={jobType} variant='icon' />
            )}
            <div className='min-w-0'>
              <div className='flex items-center gap-1.5'>
                <span className='truncate font-medium'>{schedule.name}</span>
                {schedule.last_error && (
                  <AlertTriangle
                    className='h-3.5 w-3.5 shrink-0 text-destructive'
                    aria-label='Last run failed'
                  />
                )}
                {schedule.missed_runs > 0 && (
                  <Badge variant='outline' className='shrink-0'>
                    {schedule.missed_runs} missed
                  </Badge>
                )}
              </div>
              <span className='truncate font-mono text-muted-foreground text-xs'>
                {schedule.target_ref}
              </span>
            </div>
          </div>
        </TableCell>

        {/* Schedule */}
        <TableCell data-label='Schedule'>
          <div className='flex flex-col'>
            <code className='font-mono text-sm'>{schedule.cron_expr}</code>
            <span className='text-muted-foreground text-xs'>
              {describeCron(schedule.cron_expr)} · {schedule.timezone}
            </span>
          </div>
        </TableCell>

        {/* Next run */}
        <TableCell data-label='Next run'>
          {schedule.enabled ? (
            <span title={formatTimestamp(schedule.next_run_at)} className='text-sm'>
              {formatRelative(schedule.next_run_at)}
            </span>
          ) : (
            <span className='text-muted-foreground text-sm'>Paused</span>
          )}
        </TableCell>

        {/* Last run */}
        <TableCell data-label='Last run'>
          {lastStatus ? (
            <div className='flex items-center gap-2'>
              <StatusBadge status={lastStatus} iconOnly />
              <span
                className='text-muted-foreground text-xs'
                title={formatTimestamp(schedule.last_fired_at)}
              >
                {formatRelative(schedule.last_fired_at)}
              </span>
            </div>
          ) : (
            <span className='text-muted-foreground text-sm'>Never run</span>
          )}
        </TableCell>

        {/* Enabled toggle */}
        <TableCell data-label='On' onClick={stop}>
          <Switch
            checked={schedule.enabled}
            onCheckedChange={toggleEnabled}
            disabled={!canManage || update.isPending}
          />
        </TableCell>

        {/* Actions */}
        <TableCell onClick={stop}>
          <DropdownMenu>
            <DropdownMenuTrigger
              data-testid='coordinator-job-action-menu'
              className={cn(
                "inline-flex h-7 w-7 items-center justify-center ",
                "text-muted-foreground hover:bg-muted hover:text-foreground"
              )}
              disabled={!canManage}
            >
              <MoreHorizontal className='h-4 w-4' />
            </DropdownMenuTrigger>
            <DropdownMenuContent align='end' className={DESIGN_REFERENCE}>
              <DropdownMenuItem onClick={() => setRunOpen(true)}>
                <Play className='h-4 w-4' />
                Run now
              </DropdownMenuItem>
              <DropdownMenuItem onClick={() => setEditOpen(true)}>
                <Pencil className='h-4 w-4' />
                Edit
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem onClick={() => setDeleteOpen(true)} className='text-destructive'>
                <Trash2 className='h-4 w-4' />
                Delete
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </TableCell>
      </TableRow>

      <ScheduleDialog open={editOpen} onOpenChange={setEditOpen} schedule={schedule} />
      <RunNowDialog open={runOpen} onOpenChange={setRunOpen} schedule={schedule} />
      <DeleteJobDialog
        open={deleteOpen}
        onOpenChange={setDeleteOpen}
        schedule={schedule}
        onConfirm={async () => {
          await del.mutateAsync(schedule.id);
          setDeleteOpen(false);
        }}
      />
    </>
  );
};
