import { AxiosError, type AxiosResponse } from "axios";
import type {
  AbortRequest,
  Design,
  ExperimentDesignInput,
  ExperimentListItem,
  InterimOutcome,
  InterimResponse,
  PriceRequest,
  PriceResponse,
  Proposal,
  RegisterResponse,
  ResultOutcome,
  ResultResponse,
  RunStatus,
  WriteBackResponse
} from "@/types/experiment";
import { apiClient } from "./axios";

/**
 * An error body's message. The backend's `ApiError` is `(StatusCode, String)` —
 * a plain-text body — while PR 5's 404 is `{ error }` and older routes use
 * `{ message }`; all three are read here.
 */
function messageOf(data: unknown): string | null {
  if (typeof data === "string" && data.trim() !== "") return data;
  if (data && typeof data === "object") {
    const body = data as { message?: unknown; error?: unknown };
    if (typeof body.message === "string" && body.message) return body.message;
    if (typeof body.error === "string" && body.error) return body.error;
  }
  return null;
}

function rethrow(error: unknown, action: string): never {
  if (error instanceof AxiosError) {
    const message = messageOf(error.response?.data);
    if (message) throw new Error(message);
    // A guard rejection can carry no body; axios' own text would read "status code 403".
    if (error.response?.status === 403) {
      throw new Error(`You don't have permission to ${action}.`);
    }
  }
  throw error;
}

/**
 * Deliberately no `branch` anywhere in this file. Sending the IDE branch makes
 * the server route the request to the ide singleton and skips the compile
 * boundary; these routes read persisted rows (or enqueue jobs), so they answer
 * from any replica.
 */
const experimentsPath = (projectId: string) => `/${projectId}/experiments`;
const experimentPath = (projectId: string, name: string) =>
  `${experimentsPath(projectId)}/${encodeURIComponent(name)}`;

/** Client for the `/experiments*` endpoints. */
export class ExperimentService {
  /** Every experiment record, with its status. */
  static async list(projectId: string): Promise<ExperimentListItem[]> {
    try {
      const response = await apiClient.get<ExperimentListItem[]>(experimentsPath(projectId));
      return response.data;
    } catch (error) {
      rethrow(error, "list experiments");
    }
  }

  /** Queue a pricing job. The row exists (`pending`) by the time this returns. */
  static async price(
    projectId: string,
    name: string,
    request: PriceRequest
  ): Promise<PriceResponse> {
    try {
      const response = await apiClient.post<PriceResponse>(
        `${experimentPath(projectId, name)}/price`,
        request
      );
      return response.data;
    } catch (error) {
      rethrow(error, "price this experiment");
    }
  }

  /** The proposal log, newest first. */
  static async getProposals(projectId: string, name: string): Promise<Proposal[]> {
    try {
      const response = await apiClient.get<Proposal[]>(
        `${experimentPath(projectId, name)}/proposals`
      );
      return response.data;
    } catch (error) {
      rethrow(error, "view the proposals");
    }
  }

  /** Operations view of a run. */
  static async getRun(projectId: string, name: string): Promise<RunStatus> {
    try {
      const response = await apiClient.get<RunStatus>(`${experimentPath(projectId, name)}/run`);
      return response.data;
    } catch (error) {
      rethrow(error, "view this run");
    }
  }

  /** Record an abort (202, with the run's status); the tick runs the off lever, ends the run and notifies the owner. */
  static async abort(projectId: string, name: string, request: AbortRequest): Promise<RunStatus> {
    try {
      const response = await apiClient.post<RunStatus>(
        `${experimentPath(projectId, name)}/abort`,
        request
      );
      return response.data;
    } catch (error) {
      rethrow(error, "abort this experiment");
    }
  }

  /**
   * An interim reading of a live / closing run. A 409 `{ in_progress: false }`
   * (the run is not live / closing) is the server declining, returned as a value.
   */
  static async getInterim(projectId: string, name: string): Promise<InterimOutcome> {
    let res: AxiosResponse<unknown>;
    try {
      res = await apiClient.get(`${experimentPath(projectId, name)}/interim`, {
        validateStatus: (s) => (s >= 200 && s < 300) || s === 409
      });
    } catch (error) {
      rethrow(error, "read the interim estimate");
    }

    if (res.status === 409) {
      const body = res.data as
        | { in_progress?: unknown; state?: unknown; error?: unknown }
        | undefined;
      if (body?.in_progress === false) {
        return {
          kind: "not_in_progress",
          state: typeof body.state === "string" ? body.state : "",
          error: typeof body.error === "string" ? body.error : null
        };
      }
      throw new Error(messageOf(res.data) ?? "The interim request was refused (409).");
    }
    return { kind: "interim", interim: res.data as InterimResponse };
  }

  /** Enqueue a fresh interim reading (no body); a no-op server-side while one is running. */
  static async requestInterim(projectId: string, name: string): Promise<InterimResponse> {
    try {
      const response = await apiClient.post<InterimResponse>(
        `${experimentPath(projectId, name)}/interim`
      );
      return response.data;
    } catch (error) {
      rethrow(error, "request an interim estimate");
    }
  }

  /**
   * The result, `sealed`, or `pending`.
   *
   * Both 409s are the server declining, not failing: `{ sealed: true, state }`
   * until the run has reported, `{ sealed: false, pending: true, state }` for a
   * terminal run whose row is not written yet. They are accepted here and
   * returned as values. Any other 409 throws with its own message.
   */
  static async getResult(projectId: string, name: string): Promise<ResultOutcome> {
    let res: AxiosResponse<unknown>;
    try {
      res = await apiClient.get(`${experimentPath(projectId, name)}/result`, {
        validateStatus: (s) => (s >= 200 && s < 300) || s === 409
      });
    } catch (error) {
      rethrow(error, "view this result");
    }

    if (res.status === 409) {
      const body = res.data as { sealed?: unknown; pending?: unknown; state?: unknown } | undefined;
      const state = typeof body?.state === "string" ? body.state : "";
      if (body?.sealed === true) return { kind: "sealed", state };
      if (
        body?.sealed === false &&
        body.pending === true &&
        (state === "reported" || state === "aborted")
      ) {
        return { kind: "pending", state };
      }
      throw new Error(messageOf(res.data) ?? "The result request was refused (409).");
    }
    return { kind: "report", result: res.data as ResultResponse };
  }

  /** Create the review branch carrying the coefficient patch. `IdeOnly` on the server. */
  static async applyWriteBack(projectId: string, name: string): Promise<WriteBackResponse> {
    try {
      const response = await apiClient.post<WriteBackResponse>(
        `${experimentPath(projectId, name)}/write-back/apply`
      );
      return response.data;
    } catch (error) {
      rethrow(error, "propose changes to the semantic model");
    }
  }

  /** Create an experiment and its draft design. 409 when the name exists, 422 when invalid. */
  static async create(projectId: string, config: ExperimentDesignInput): Promise<Design> {
    try {
      const response = await apiClient.post<Design>(experimentsPath(projectId), { config });
      return response.data;
    } catch (error) {
      rethrow(error, "create an experiment");
    }
  }

  /** The experiment's one design: a draft until registered, then frozen. */
  static async getDesign(projectId: string, name: string): Promise<Design> {
    try {
      const response = await apiClient.get<Design>(`${experimentPath(projectId, name)}/design`);
      return response.data;
    } catch (error) {
      rethrow(error, "view the design");
    }
  }

  /**
   * Update the draft in place. 409 = the design is frozen, or `baseHash` is no longer
   * the current content hash (someone saved first). 422 = invalid.
   */
  static async saveDesign(
    projectId: string,
    name: string,
    config: ExperimentDesignInput,
    baseHash?: string
  ): Promise<Design> {
    try {
      const response = await apiClient.put<Design>(
        `${experimentPath(projectId, name)}/design`,
        baseHash ? { config, base_hash: baseHash } : { config }
      );
      return response.data;
    } catch (error) {
      rethrow(error, "edit this design");
    }
  }

  /** Copy the design as a new draft named `newName`; returns the new experiment's design. 409 when the name is taken. */
  static async duplicate(projectId: string, name: string, newName: string): Promise<Design> {
    try {
      const response = await apiClient.post<Design>(
        `${experimentPath(projectId, name)}/duplicate`,
        { name: newName }
      );
      return response.data;
    } catch (error) {
      rethrow(error, "duplicate this experiment");
    }
  }

  /** Adopt a priced proposal into the draft (updated in place). 409 when the proposal is stale. */
  static async adoptProposal(projectId: string, name: string, proposalId: string): Promise<Design> {
    try {
      const response = await apiClient.post<Design>(
        `${experimentPath(projectId, name)}/proposals/${encodeURIComponent(proposalId)}/use`
      );
      return response.data;
    } catch (error) {
      rethrow(error, "use this design");
    }
  }

  /** Freeze the design as the run. Owner/Admin only; irreversible. 409 when `contentHash` is no longer current. */
  static async register(
    projectId: string,
    name: string,
    contentHash: string
  ): Promise<RegisterResponse> {
    try {
      const response = await apiClient.post<RegisterResponse>(
        `${experimentPath(projectId, name)}/register`,
        { content_hash: contentHash }
      );
      return response.data;
    } catch (error) {
      rethrow(error, "register this experiment");
    }
  }

  /** Analyse a design whose dates are all past from existing data (no levers move). Owner/Admin only. */
  static async retrospective(
    projectId: string,
    name: string,
    contentHash: string
  ): Promise<RegisterResponse> {
    try {
      const response = await apiClient.post<RegisterResponse>(
        `${experimentPath(projectId, name)}/retrospective`,
        { content_hash: contentHash }
      );
      return response.data;
    } catch (error) {
      rethrow(error, "analyse this experiment");
    }
  }
}
