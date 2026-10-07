import { type Query, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import {
  cannotCompileMessage,
  isPreviewNotFound,
  sampleRunErrorMessage,
  saveSourceErrorMessage,
  startRunErrorMessage
} from "@/libs/utils/preview";
import { PreviewService } from "@/services/api/previews";
import type {
  PreviewChecksResponse,
  PreviewRunDetail,
  PreviewRunSummary,
  PreviewSourceItem,
  StartPreviewRunRequest,
  UpsertPreviewSourceRequest,
  WorkspacePreview
} from "@/types/workspace";
import { errMessage } from "../errMessage";
import queryKeys from "../queryKey";

/** How often the list is re-read while any preview is still compiling. */
export const PREVIEW_POLL_MS = 2_000;

/**
 * Poll while anything is still changing on its own, and stop the moment
 * nothing is: a compile in flight, or — separately — a row's checks analysis
 * still running (`checks.status === "pending"`), which can outlast the
 * compile. The Checks panel has no poll of its own for the row-level summary
 * (only its expanded per-pipeline detail does); this list poll is the only
 * thing that ever moves it off "checking…". `ready`/`failed` compiles and
 * settled checks only change when someone acts, and those actions refresh
 * the list themselves.
 */
function previewsRefetchInterval(
  query: Query<WorkspacePreview[], Error, WorkspacePreview[], readonly unknown[]>
): number | false {
  const stillMoving = (p: WorkspacePreview) =>
    p.status === "compiling" || p.checks?.status === "pending";
  return query.state.data?.some(stillMoving) ? PREVIEW_POLL_MS : false;
}

export const usePreviews = (workspaceId: string | undefined, enabled = true) =>
  useQuery<WorkspacePreview[], Error, WorkspacePreview[], readonly unknown[]>({
    queryKey: queryKeys.workspaces.previews(workspaceId ?? ""),
    queryFn: () => PreviewService.list(workspaceId ?? ""),
    enabled: enabled && !!workspaceId,
    refetchInterval: previewsRefetchInterval
  });

/** The row for one branch, straight from the (possibly polling) list. */
export const usePreview = (workspaceId: string | undefined, branch: string | null) => {
  const query = usePreviews(workspaceId, !!branch);
  const preview = branch ? query.data?.find((p) => p.branch === branch) : undefined;
  return { ...query, preview };
};

/**
 * Write the row the server just returned into the cached list, so the table
 * (and the polling that keys off it) moves the moment the `202` lands rather
 * than on the next refetch.
 */
function upsert(list: WorkspacePreview[] | undefined, item: WorkspacePreview): WorkspacePreview[] {
  const rows = list ?? [];
  return rows.some((p) => p.branch === item.branch)
    ? rows.map((p) => (p.branch === item.branch ? item : p))
    : [item, ...rows];
}

function usePreviewMutation(
  workspaceId: string,
  mutationFn: (branch: string) => Promise<WorkspacePreview>,
  failure: string
) {
  const queryClient = useQueryClient();
  const key = queryKeys.workspaces.previews(workspaceId);
  return useMutation({
    mutationFn,
    onSuccess: (item) => {
      // Written as returned: a refresh of an unchanged head answers with the
      // existing READY revision, so the row stays ready and nothing polls.
      queryClient.setQueryData<WorkspacePreview[]>(key, (list) => upsert(list, item));
      queryClient.invalidateQueries({ queryKey: key });
    },
    onError: (err) => {
      // These two are said where the person asked (the row, the New preview
      // field, the IDE button), in the server's words — a toast would repeat
      // them somewhere else, generically.
      if (cannotCompileMessage(err) || isPreviewNotFound(err)) return;
      toast.error(errMessage(err, failure));
    }
  });
}

export const useCreatePreview = (workspaceId: string) =>
  usePreviewMutation(
    workspaceId,
    (branch) => PreviewService.create(workspaceId, branch),
    "Failed to create the preview."
  );

export const useRefreshPreview = (workspaceId: string) =>
  usePreviewMutation(
    workspaceId,
    (branch) => PreviewService.refresh(workspaceId, branch),
    "Failed to refresh the preview."
  );

export const useDeletePreview = (workspaceId: string) => {
  const queryClient = useQueryClient();
  const key = queryKeys.workspaces.previews(workspaceId);
  return useMutation({
    mutationFn: (branch: string) => PreviewService.remove(workspaceId, branch),
    onSuccess: (_, branch) => {
      queryClient.setQueryData<WorkspacePreview[]>(key, (list) =>
        (list ?? []).filter((p) => p.branch !== branch)
      );
      queryClient.invalidateQueries({ queryKey: key });
    },
    onError: (err) => toast.error(errMessage(err, "Failed to delete the preview."))
  });
};

// ── Checks (S12) ─────────────────────────────────────────────────────────

/** How often a still-analyzing check run is re-read. */
const PREVIEW_CHECKS_POLL_MS = 2_000;

/**
 * The per-pipeline detail behind a row's embedded `checks` summary. Polls
 * while the analysis is still running (`status: "pending"`) and stops once
 * it settles (`"done"` or `"failed"`).
 */
export const usePreviewChecks = (
  workspaceId: string | undefined,
  branch: string | undefined,
  enabled = true
) =>
  useQuery<PreviewChecksResponse, Error>({
    queryKey: queryKeys.workspaces.previewChecks(workspaceId ?? "", branch ?? ""),
    queryFn: () => PreviewService.checks(workspaceId ?? "", branch ?? ""),
    enabled: enabled && !!workspaceId && !!branch,
    refetchInterval: (query) =>
      query.state.data?.status === "pending" ? PREVIEW_CHECKS_POLL_MS : false
  });

// ── Held procedure runs (S12) ───────────────────────────────────────────

/** How often the runs list / a single run is re-read while one is in flight. */
const PREVIEW_RUN_POLL_MS = 2_000;

const RUN_IN_FLIGHT = new Set(["queued", "running"]);

/** Runs for a branch, newest first. Polls while any listed run hasn't finished. */
export const usePreviewRuns = (workspaceId: string | undefined, branch: string | undefined) =>
  useQuery<PreviewRunSummary[], Error>({
    queryKey: queryKeys.workspaces.previewRuns(workspaceId ?? "", branch ?? ""),
    queryFn: () => PreviewService.listRuns(workspaceId ?? "", branch ?? ""),
    enabled: !!workspaceId && !!branch,
    refetchInterval: (query) =>
      query.state.data?.some((run) => RUN_IN_FLIGHT.has(run.state)) ? PREVIEW_RUN_POLL_MS : false
  });

/**
 * One run's detail (steps included). Polls while the run itself hasn't
 * finished, OR — a `transform_build`'s linked `compare` can still be
 * queued/running after the build itself finishes — while its `compare` is
 * still going: otherwise this stops the moment the build settles and the
 * embedded compare view freezes on "Comparing…" until something else happens
 * to re-fetch it.
 */
export const usePreviewRun = (workspaceId: string | undefined, runId: string | undefined) =>
  useQuery<PreviewRunDetail, Error>({
    queryKey: queryKeys.workspaces.previewRun(workspaceId ?? "", runId ?? ""),
    queryFn: () => PreviewService.getRun(workspaceId ?? "", runId ?? ""),
    enabled: !!workspaceId && !!runId,
    refetchInterval: (query) => {
      const data = query.state.data;
      const inFlight =
        RUN_IN_FLIGHT.has(data?.state ?? "") || RUN_IN_FLIGHT.has(data?.compare?.state ?? "");
      return inFlight ? PREVIEW_RUN_POLL_MS : false;
    }
  });

/**
 * Start a held run — a procedure dry-run or an Airway sample, the two forms
 * that share this mutation. Each form maps the refusals it can provoke to
 * inline text of its own (`startRunErrorMessage` / `sampleRunErrorMessage`)
 * and those stay quiet here — same split as `usePreviewMutation` above
 * between "shown where asked" and "toasted" — so this checks both mappers
 * rather than assuming which kind of request just failed.
 */
export const useStartPreviewRun = (workspaceId: string) => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (request: StartPreviewRunRequest) => PreviewService.startRun(workspaceId, request),
    onSuccess: (_, request) => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.workspaces.previewRuns(workspaceId, request.branch)
      });
    },
    onError: (err) => {
      if (startRunErrorMessage(err) || sampleRunErrorMessage(err)) return;
      toast.error(errMessage(err, "Failed to start the run."));
    }
  });
};

// ── Sandbox sources (S11 Airway samples) ────────────────────────────────

/**
 * Registered sandbox sources for the workspace, by pipeline. Not gated by
 * `OXY_PREVIEW_RUNS` (registering a source is setup, not a run), and not
 * branch-scoped — one sandbox company per pipeline serves every branch.
 */
export const usePreviewSources = (workspaceId: string | undefined) =>
  useQuery<PreviewSourceItem[], Error>({
    queryKey: queryKeys.workspaces.previewSources(workspaceId ?? ""),
    queryFn: () => PreviewService.listSources(workspaceId ?? ""),
    enabled: !!workspaceId
  });

/**
 * Register or edit a pipeline's sandbox source. The three save refusals
 * (`saveSourceErrorMessage`) are shown on the form itself, same split as
 * `usePreviewMutation` above between "shown where asked" and "toasted".
 */
export const useUpsertPreviewSource = (workspaceId: string) => {
  const queryClient = useQueryClient();
  const key = queryKeys.workspaces.previewSources(workspaceId);
  return useMutation({
    mutationFn: (request: UpsertPreviewSourceRequest) =>
      PreviewService.upsertSource(workspaceId, request),
    onSuccess: (item) => {
      queryClient.setQueryData<PreviewSourceItem[]>(key, (list) => {
        const rows = list ?? [];
        return rows.some((s) => s.pipeline === item.pipeline)
          ? rows.map((s) => (s.pipeline === item.pipeline ? item : s))
          : [...rows, item];
      });
      queryClient.invalidateQueries({ queryKey: key });
    },
    onError: (err) => {
      if (saveSourceErrorMessage(err)) return;
      toast.error(errMessage(err, "Failed to save the sandbox source."));
    }
  });
};
