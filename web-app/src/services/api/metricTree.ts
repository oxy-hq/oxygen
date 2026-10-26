import { AxiosError } from "axios";
import type {
  BaselineRequest,
  BaselineResponse,
  CohortRequest,
  CohortResponse,
  DistributionRequest,
  DrillRequest,
  DrillResponse,
  ExplainRequest,
  ExplainResult,
  MetricTree,
  OpportunityRequest,
  OpportunityResult,
  PredictChange,
  PredictOptions,
  PredictResult,
  ProjectionRequest,
  ProjectionResponse,
  SensitivityResult,
  TimeDimensionsResponse
} from "@/types/metricTree";
import { apiClient } from "./axios";

function rethrow(error: unknown): never {
  if (error instanceof AxiosError && error.response?.data?.message) {
    throw new Error(error.response.data.message);
  }
  throw error;
}

/**
 * An API error that kept the server's `code`.
 *
 * `rethrow` above flattens an error to its message, which is right for
 * endpoints whose failures are all "something went wrong". The cohort endpoint
 * is not one of those: `cohort_not_resolved` means the measure declares no
 * `default_cohort:` — an authoring gap the panel renders as an honest empty
 * state — and it is indistinguishable from a real failure once the code is
 * dropped. Matching on the message text instead would bind the UI to prose.
 */
export class ApiCodedError extends Error {
  readonly code?: string;

  constructor(message: string, code?: string) {
    super(message);
    this.name = "ApiCodedError";
    this.code = code;
  }
}

function rethrowWithCode(error: unknown): never {
  if (error instanceof AxiosError && error.response?.data?.message) {
    throw new ApiCodedError(error.response.data.message, error.response.data.code);
  }
  throw error;
}

/** Client for the `/semantic/metric-tree*` endpoints. */
export class MetricTreeService {
  /** The full metric tree, or the subtree rooted at `root` when provided. */
  static async getTree(projectId: string, root?: string, branchName?: string): Promise<MetricTree> {
    try {
      const response = await apiClient.get<MetricTree>(`/${projectId}/semantic/metric-tree`, {
        params: {
          ...(root ? { root } : {}),
          ...(branchName ? { branch: branchName } : {})
        }
      });
      return response.data;
    } catch (error) {
      rethrow(error);
    }
  }

  /** Ranked drivers of a measure. */
  static async getSensitivity(
    projectId: string,
    measureId: string,
    branchName?: string
  ): Promise<SensitivityResult> {
    try {
      const response = await apiClient.get<SensitivityResult>(
        `/${projectId}/semantic/metric-tree/${encodeURIComponent(measureId)}/sensitivity`,
        { params: branchName ? { branch: branchName } : {} }
      );
      return response.data;
    } catch (error) {
      rethrow(error);
    }
  }

  /** Propagate hypothetical `(measure, delta)` changes upward through the tree.
   *  Passing `values` sizes multiplicative edges that would otherwise come
   *  back `unquantifiable`. */
  static async predict(
    projectId: string,
    changes: PredictChange[],
    branchName?: string,
    // One object rather than two more positional optionals: they are the same
    // pair the SDK's `predict` takes, and `PredictOptions` is where their
    // meaning is documented — a caller passing them positionally had to read
    // this signature to learn that omitting `coefficients` silently drops every
    // undeclared edge from the result.
    options: PredictOptions = {}
  ): Promise<PredictResult> {
    const { values, coefficients } = options;
    try {
      const response = await apiClient.post<PredictResult>(
        `/${projectId}/semantic/metric-tree/predict`,
        {
          changes,
          ...(values ? { values } : {}),
          // Sent verbatim, refusals included — the server ignores entries
          // carrying no coefficient, and filtering them here would just be a
          // second place for the two sides to disagree.
          ...(coefficients?.length ? { coefficients } : {})
        },
        { params: branchName ? { branch: branchName } : {} }
      );
      return response.data;
    } catch (error) {
      rethrow(error);
    }
  }

  /** Current values for the levers plus everything downstream of them. */
  static async baseline(
    projectId: string,
    request: BaselineRequest,
    branchName?: string
  ): Promise<BaselineResponse> {
    try {
      const response = await apiClient.post<BaselineResponse>(
        `/${projectId}/semantic/metric-tree/baseline`,
        request,
        { params: branchName ? { branch: branchName } : {} }
      );
      return response.data;
    } catch (error) {
      rethrow(error);
    }
  }

  /** Bucketed history plus a forward forecast for the levers and everything
   *  downstream of them. Returns the BASELINE curve only — the scenario curve
   *  is composed client-side from this and the `predict` result, so editing a
   *  lever costs no warehouse query. */
  static async projection(
    projectId: string,
    request: ProjectionRequest,
    branchName?: string
  ): Promise<ProjectionResponse> {
    try {
      const response = await apiClient.post<ProjectionResponse>(
        `/${projectId}/semantic/metric-tree/projection`,
        request,
        { params: branchName ? { branch: branchName } : {} }
      );
      return response.data;
    } catch (error) {
      rethrow(error);
    }
  }

  /** Period-over-period root-cause decomposition. */
  static async explain(
    projectId: string,
    request: ExplainRequest,
    branchName?: string
  ): Promise<ExplainResult> {
    try {
      const response = await apiClient.post<ExplainResult>(
        `/${projectId}/semantic/metric-tree/explain`,
        request,
        { params: branchName ? { branch: branchName } : {} }
      );
      return response.data;
    } catch (error) {
      rethrow(error);
    }
  }

  /** Time dimensions available per view (`view.dim` ids). */
  static async timeDimensions(
    projectId: string,
    branchName?: string
  ): Promise<TimeDimensionsResponse> {
    try {
      const response = await apiClient.get<TimeDimensionsResponse>(
        `/${projectId}/semantic/metric-tree/time-dimensions`,
        { params: branchName ? { branch: branchName } : {} }
      );
      return response.data;
    } catch (error) {
      rethrow(error);
    }
  }

  /** Single-period distribution; baseline auto-derived server-side. */
  static async distribution(
    projectId: string,
    request: DistributionRequest,
    branchName?: string
  ): Promise<ExplainResult> {
    try {
      const response = await apiClient.post<ExplainResult>(
        `/${projectId}/semantic/metric-tree/distribution`,
        request,
        { params: branchName ? { branch: branchName } : {} }
      );
      return response.data;
    } catch (error) {
      rethrow(error);
    }
  }

  /** Segment opportunity sizing for a measure over a period. */
  static async opportunity(
    projectId: string,
    request: OpportunityRequest,
    branchName?: string
  ): Promise<OpportunityResult> {
    try {
      const response = await apiClient.post<OpportunityResult>(
        `/${projectId}/semantic/metric-tree/opportunity`,
        request,
        { params: branchName ? { branch: branchName } : {} }
      );
      return response.data;
    } catch (error) {
      rethrow(error);
    }
  }

  /** Recursive opportunity decomposition for a measure over a period. */
  static async drill(projectId: string, request: DrillRequest): Promise<DrillResponse> {
    try {
      const response = await apiClient.post<DrillResponse>(
        `/${projectId}/semantic/metric-tree/drill`,
        request
      );
      return response.data;
    } catch (error) {
      rethrow(error);
    }
  }

  /**
   * Peer-cohort comparison: this entity's instances against their declared
   * peers on one measure.
   *
   * Not under `/metric-tree` — it is a sibling semantic op, mounted at
   * `/semantic/cohort` alongside `/semantic` itself. Throws
   * {@link ApiCodedError} so a caller can tell `cohort_not_resolved` (nothing
   * authored yet) from a real failure.
   */
  static async cohort(
    projectId: string,
    request: CohortRequest,
    branchName?: string
  ): Promise<CohortResponse> {
    try {
      const response = await apiClient.post<CohortResponse>(
        `/${projectId}/semantic/cohort`,
        request,
        { params: branchName ? { branch: branchName } : {} }
      );
      return response.data;
    } catch (error) {
      rethrowWithCode(error);
    }
  }
}
