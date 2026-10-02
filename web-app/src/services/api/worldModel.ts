import type {
  WmFilterCountEvent,
  WmInstanceDetailEvent,
  WmInstancesResponse,
  WmMeasureBreakdownEvent,
  WorldModel
} from "@/types/worldModel";
import { apiBaseURL } from "../env";
import { apiClient } from "./axios";
import fetchSSE from "./fetchSSE";

const toError = (error: unknown): Error =>
  error instanceof Error ? error : new Error(String(error));

// Every method threads the IDE's selected branch through as `?branch=` so the
// backend's workspace middleware resolves the branch worktree instead of the
// workspace root (see effective_workspace_path). Empty/undefined falls back to
// the default branch server-side.
export class WorldModelService {
  static async getWorldModel(projectId: string, branch?: string): Promise<WorldModel> {
    const response = await apiClient.get<WorldModel>(`/${projectId}/semantic/world-model`, {
      params: { branch: branch || undefined }
    });
    return response.data;
  }

  static async getInstances(
    projectId: string,
    entityId: string,
    search?: string,
    limit = 50,
    branch?: string
  ): Promise<WmInstancesResponse> {
    const response = await apiClient.get<WmInstancesResponse>(
      `/${projectId}/semantic/world-model/instances`,
      {
        params: {
          entity: entityId,
          search: search || undefined,
          limit,
          branch: branch || undefined
        }
      }
    );
    return response.data;
  }

  /**
   * Paginated, searchable listing of the rows of `entityId` reachable from the
   * selected instance (`seedEntityId` / `seedKey`) — the full set the node card
   * only previews as a few sample chips.
   */
  static async getFilterInstances(
    projectId: string,
    seedEntityId: string,
    seedKey: string,
    entityId: string,
    search?: string,
    limit = 50,
    offset = 0,
    branch?: string
  ): Promise<WmInstancesResponse> {
    const response = await apiClient.get<WmInstancesResponse>(
      `/${projectId}/semantic/world-model/filter-instances`,
      {
        params: {
          seed_entity: seedEntityId,
          seed_key: seedKey,
          entity: entityId,
          search: search || undefined,
          limit,
          offset: offset || undefined,
          branch: branch || undefined
        }
      }
    );
    return response.data;
  }

  static streamFilterCounts(
    projectId: string,
    entityId: string,
    keyValue: string,
    onEvent: (event: WmFilterCountEvent) => void,
    onClose: () => void,
    onError: (error: Error) => void,
    signal: AbortSignal,
    branch?: string
  ): void {
    // Branch rides the query string — the workspace middleware only reads it
    // from there, the POST body is the handler's own payload.
    const qs = branch ? `?${new URLSearchParams({ branch })}` : "";
    // `fetchSSE` reports a failure twice: it calls its `onError`, then rejects (after
    // logging). The rejection is the one forwarded — it also covers a throw before the
    // request starts — so `fetchSSE`'s own `onError` is left unwired. A failed stream
    // reaches the caller's `onError` and never `onClose`; an aborted one reaches neither.
    fetchSSE<WmFilterCountEvent>(
      `${apiBaseURL}/${projectId}/semantic/world-model/filter-counts${qs}`,
      {
        method: "POST",
        body: { entity_id: entityId, key_value: keyValue },
        onMessage: onEvent,
        onClose,
        signal
      }
    ).catch((error: unknown) => onError(toError(error)));
  }

  static streamInstanceDetail(
    projectId: string,
    entityId: string,
    keyValue: string,
    onEvent: (event: WmInstanceDetailEvent) => void,
    onClose: () => void,
    onError: (error: Error) => void,
    signal: AbortSignal,
    branch?: string
  ): void {
    const params = new URLSearchParams({ entity: entityId, key: keyValue });
    if (branch) params.set("branch", branch);
    // A failed stream is reported through the rejection — see `streamFilterCounts`.
    fetchSSE<WmInstanceDetailEvent>(
      `${apiBaseURL}/${projectId}/semantic/world-model/instance-detail?${params}`,
      { method: "GET", onMessage: onEvent, onClose, signal }
    ).catch((error: unknown) => onError(toError(error)));
  }

  static streamMeasureBreakdown(
    projectId: string,
    entityId: string,
    keyValue: string,
    measure: string,
    onEvent: (event: WmMeasureBreakdownEvent) => void,
    onClose: () => void,
    onError: (error: Error) => void,
    signal: AbortSignal,
    branch?: string
  ): void {
    const params = new URLSearchParams({ entity: entityId, key: keyValue, measure });
    if (branch) params.set("branch", branch);
    // A failed stream is reported through the rejection — see `streamFilterCounts`.
    fetchSSE<WmMeasureBreakdownEvent>(
      `${apiBaseURL}/${projectId}/semantic/world-model/measure-breakdown?${params}`,
      { method: "GET", onMessage: onEvent, onClose, signal }
    ).catch((error: unknown) => onError(toError(error)));
  }
}
