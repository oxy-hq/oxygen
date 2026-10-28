import { AnomaliesClient } from "./anomalies";
import { createConfigAsync, type OxyConfig } from "./config";
import { MetricTreeClient } from "./metricTree";
import { PeerCohortClient } from "./peerCohort";
import type { ApiError } from "./types";

/**
 * Oxy API Client for interacting with Oxy data
 */
export class OxyClient {
  private config: OxyConfig;
  private _metricTree: MetricTreeClient | undefined;
  private _anomalies: AnomaliesClient | undefined;
  private _peerCohort: PeerCohortClient | undefined;

  constructor(config: OxyConfig) {
    this.config = config;
  }

  /**
   * Metric-tree analyses: tree introspection, sensitivity, predict, explain,
   * and opportunity sizing. Lazily constructed on first access.
   *
   * @example
   * ```typescript
   * const tree = await client.metricTree.getTree();
   * const sens = await client.metricTree.getSensitivity("orders.net_revenue");
   * ```
   */
  get metricTree(): MetricTreeClient {
    if (!this._metricTree) {
      this._metricTree = new MetricTreeClient(
        this.config,
        // Bind so the helper keeps `this` even when destructured by the inner client.
        this.request.bind(this)
      );
    }
    return this._metricTree;
  }

  /**
   * Anomaly inbox: list / scan / update-status / explain. Lazily constructed
   * on first access. Pairs with `metricTree.explain` — anomaly rows carry a
   * cached `ExplainResult` populated by `anomalies.explain(id)`.
   *
   * @example
   * ```typescript
   * await client.anomalies.scan({ as_of: "2025-12-15" });
   * const { anomalies } = await client.anomalies.list({ status: "new" });
   * ```
   */
  get anomalies(): AnomaliesClient {
    if (!this._anomalies) {
      this._anomalies = new AnomaliesClient(this.config, this.request.bind(this));
    }
    return this._anomalies;
  }

  /**
   * Peer-cohort benchmarking: resolve a subject's peer cohort and every
   * peer's comparison against it. Lazily constructed on first access.
   *
   * @example
   * ```typescript
   * const result = await client.peerCohort.resolve({
   *   entity: "restaurant_id",
   *   measure: "orders.net_revenue",
   *   time_dimension: "orders.order_date",
   *   period: ["2025-09-01", "2025-09-30"],
   * });
   * ```
   */
  get peerCohort(): PeerCohortClient {
    if (!this._peerCohort) {
      this._peerCohort = new PeerCohortClient(this.config, this.request.bind(this));
    }
    return this._peerCohort;
  }

  /**
   * Creates an OxyClient instance asynchronously with support for postMessage authentication
   *
   * This is the recommended method when using the SDK in an iframe that needs to
   * obtain authentication from the parent window via postMessage.
   *
   * @param config - Optional configuration overrides
   * @returns Promise resolving to OxyClient instance
   * @throws Error if required configuration is missing
   * @throws PostMessageAuthTimeoutError if parent doesn't respond
   *
   * @example
   * ```typescript
   * // In an iframe - automatic postMessage auth
   * const client = await OxyClient.create({
   *   parentOrigin: 'https://app.example.com',
   *   projectId: 'my-project-id',
   *   baseUrl: 'https://api.oxy.tech'
   * });
   *
   * // Use the client normally
   * const tree = await client.metricTree.getTree();
   * ```
   */
  static async create(config?: Partial<OxyConfig>): Promise<OxyClient> {
    const resolvedConfig = await createConfigAsync(config);
    return new OxyClient(resolvedConfig);
  }

  /**
   * Makes an authenticated HTTP request to the Oxy API
   */
  private async request<T>(endpoint: string, options: RequestInit = {}): Promise<T> {
    const url = `${this.config.baseUrl}${endpoint}`;

    const headers: Record<string, string> = {
      "Content-Type": "application/json",
      ...((options.headers as Record<string, string>) || {})
    };

    // Only add Authorization header if API key is provided (optional for local dev)
    if (this.config.apiKey) {
      headers.Authorization = `Bearer ${this.config.apiKey}`;
    }

    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), this.config.timeout || 30000);

    try {
      const response = await fetch(url, {
        ...options,
        headers,
        signal: controller.signal
      });

      clearTimeout(timeoutId);

      if (!response.ok) {
        const errorText = await response.text().catch(() => "Unknown error");
        const error: ApiError = {
          message: `API request failed: ${response.statusText}`,
          status: response.status,
          details: errorText
        };
        throw error;
      }

      return await response.json();
    } catch (error: unknown) {
      clearTimeout(timeoutId);

      if (error instanceof Error && error.name === "AbortError") {
        throw new Error(`Request timeout after ${this.config.timeout || 30000}ms`);
      }

      throw error;
    }
  }
}
