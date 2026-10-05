/**
 * Hooks for the `/agentic-schedules` HTTP surface + the airway file
 * list used by the schedule target picker (automation files reuse
 * `useAgenticAutomationFiles`; agent picker uses `useScheduleAgents`).
 */

import { type UseQueryResult, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { type AgentInfo, AgentService } from "@/services/api/agents";
import { type AirwayFile, AirwayService } from "@/services/api/airway";
import { ScheduleService } from "@/services/api/schedules";
import type { BackfillInput, Schedule, ScheduleInput } from "@/types/schedule";
import queryKeys from "../queryKey";

export const useSchedules = (): UseQueryResult<Schedule[]> => {
  const { project } = useCurrentProjectBranch();
  return useQuery({
    queryKey: queryKeys.schedule.list(project.id),
    queryFn: () => ScheduleService.list(project.id)
  });
};

export const useAirwayFiles = (): UseQueryResult<AirwayFile[]> => {
  const { project } = useCurrentProjectBranch();
  return useQuery({
    queryKey: queryKeys.airway.files(project.id),
    queryFn: () => AirwayService.listFiles(project.id)
  });
};

const agenticOnly = (agents: AgentInfo[]): AgentInfo[] =>
  agents.filter((a) => a.path.endsWith(".agentic.yml") || a.path.endsWith(".agentic.yaml"));

/**
 * Agents the schedule dialog can target. Filters the workspace's agent
 * list to `.agentic.yml` / `.agentic.yaml` files — the analytics
 * pipeline `start_agent_run` resolves through `PipelineBuilder.analytics`,
 * which only loads agentic configs. Classic `.agent.yml` agents go
 * through a different runtime and aren't supported as schedule targets.
 *
 * The filter is a `select`, not part of the fetch: this shares its key with
 * `useAgents`, which the top bar's clock keeps mounted beside the dialog. A
 * second `queryFn` under one key would have each overwrite the other's list
 * with its own shape.
 */
export const useScheduleAgents = (): UseQueryResult<AgentInfo[]> => {
  const { project, branchName } = useCurrentProjectBranch();
  return useQuery({
    queryKey: queryKeys.agent.list(project.id, branchName),
    queryFn: () => AgentService.listAgents(project.id, branchName),
    select: agenticOnly
  });
};

export const useCreateSchedule = () => {
  const { project } = useCurrentProjectBranch();
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (input: ScheduleInput) => ScheduleService.create(project.id, input),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: queryKeys.schedule.list(project.id) });
      toast.success("Schedule created");
    },
    onError: (e: Error) => toast.error(`Failed to create schedule: ${e.message}`)
  });
};

export const useUpdateSchedule = () => {
  const { project } = useCurrentProjectBranch();
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, input }: { id: string; input: ScheduleInput }) =>
      ScheduleService.update(project.id, id, input),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: queryKeys.schedule.list(project.id) });
      toast.success("Schedule updated");
    },
    onError: (e: Error) => toast.error(`Failed to update schedule: ${e.message}`)
  });
};

export const useDeleteSchedule = () => {
  const { project } = useCurrentProjectBranch();
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => ScheduleService.remove(project.id, id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: queryKeys.schedule.list(project.id) });
      toast.success("Schedule deleted");
    },
    onError: (e: Error) => toast.error(`Failed to delete schedule: ${e.message}`)
  });
};

export const useRunScheduleNow = () => {
  const { project } = useCurrentProjectBranch();
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => ScheduleService.runNow(project.id, id),
    onSuccess: (res) => {
      qc.invalidateQueries({ queryKey: queryKeys.schedule.list(project.id) });
      qc.invalidateQueries({ queryKey: queryKeys.coordinator.all });
      toast.success(`Run started (${res.run_id.slice(0, 8)}…)`);
    },
    onError: (e: Error) => toast.error(`Failed to run schedule: ${e.message}`)
  });
};

export const useBackfillSchedule = () => {
  const { project } = useCurrentProjectBranch();
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, input }: { id: string; input: BackfillInput }) =>
      ScheduleService.backfill(project.id, id, input),
    onSuccess: (res) => {
      qc.invalidateQueries({ queryKey: queryKeys.schedule.list(project.id) });
      // Per-job run history relies on the schedule_id filter — refresh
      // every run-history view so the new backfill rows show up.
      qc.invalidateQueries({ queryKey: queryKeys.coordinator.all });
      const seeded = res.run_ids.length;
      const partial = res.planned > seeded;
      toast.success(
        partial
          ? `Queued ${seeded} of ${res.planned} backfill runs (the rest failed — check logs)`
          : `Queued ${seeded} backfill run${seeded === 1 ? "" : "s"}`
      );
    },
    onError: (e: Error) => toast.error(`Backfill failed: ${e.message}`)
  });
};
