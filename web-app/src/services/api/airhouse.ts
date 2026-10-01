import { apiClient } from "./axios";

/**
 * Connection coordinates returned by `GET /airhouse/me/connection`.
 *
 * Notably this *does not* carry a username — the SA-backed flow doesn't
 * expose a stable per-user identity. Each token mint returns its own
 * ephemeral username on `AirhouseEphemeralToken.username`.
 */
export type AirhouseConnectionInfo = {
  host: string;
  port: number;
  /**
   * The airhouse tenant id once provisioned; empty string while
   * `is_provisioned` is `false`. Surface a CTA in that case.
   */
  dbname: string;
  /** Mapped airhouse role for the caller — `"reader" | "writer" | "admin"`. */
  role: string;
  is_provisioned: boolean;
};

/**
 * Freshly-minted ephemeral wire-protocol credential. Returned only by
 * `POST /airhouse/me/credentials`. The server does not persist these — if
 * the user navigates away without copying the password down it can't be
 * shown again. Each mint produces a distinct username/password.
 */
export type AirhouseEphemeralToken = {
  username: string;
  password: string;
  host: string;
  port: number;
  dbname: string;
  role: string;
  /** ISO-8601 timestamp; the credential stops authenticating past this. */
  expires_at: string;
};

/**
 * The running Airhouse deployment's software version. Global — there is one
 * Airhouse per deployment — so this is not scoped to a workspace, mirroring
 * how Oxy's own VersionBadge reports the running Oxy build.
 */
export type AirhouseVersionInfo = {
  version: string;
};

/** Presence + validity of one DuckLake catalog hot-path index. */
type CatalogIndexState = {
  name: string;
  present: boolean;
  /** Mirrors Postgres `indisvalid` — false while a CONCURRENTLY build runs. */
  valid: boolean;
};

/**
 * State of the workspace tenant's DuckLake catalog hot-path indexes.
 * `state` is the aggregate: `"ready"` (all present + valid), `"building"`
 * (a CONCURRENTLY build is in flight), or `"absent"` (off / not created).
 */
export type CatalogIndexesStatus = {
  state: "ready" | "building" | "absent";
  indexes: CatalogIndexState[];
};

/** Result of the toggle — airhouse applies the change asynchronously. */
export type CatalogIndexesToggleResult = {
  /** Whether airhouse accepted the async apply (its HTTP 202). */
  accepted: boolean;
};

export const AirhouseService = {
  /**
   * The running Airhouse deployment's software version. Backed by
   * `GET /airhouse/version`, which Oxy reads live from the Airhouse server's
   * public `/health`. Responds 503 when Airhouse isn't configured for the
   * deployment and 502 when its `/health` is unreachable — callers hide the
   * version badge on either, so it never shows a broken state.
   */
  async getVersion(): Promise<AirhouseVersionInfo> {
    const response = await apiClient.get("/airhouse/version");
    return response.data;
  },

  async getConnection(workspaceId: string): Promise<AirhouseConnectionInfo> {
    const response = await apiClient.get("/airhouse/me/connection", {
      params: { workspace_id: workspaceId }
    });
    return response.data;
  },

  /**
   * Mint a fresh ephemeral token via the SA-backed broker. POST because each
   * call writes an audit row on airhouse and consumes mint quota — GET
   * would violate HTTP's safe-and-idempotent contract.
   */
  async mintToken(workspaceId: string): Promise<AirhouseEphemeralToken> {
    const response = await apiClient.post("/airhouse/me/credentials", undefined, {
      params: { workspace_id: workspaceId }
    });
    return response.data;
  },

  async provision(workspaceId: string, tenantName: string): Promise<AirhouseConnectionInfo> {
    const response = await apiClient.post(
      "/airhouse/me/provision",
      { tenant_name: tenantName },
      { params: { workspace_id: workspaceId } }
    );
    return response.data;
  },

  /**
   * Revoke a single ephemeral token by its `eph_*` username. Idempotent —
   * 204 whether the token still existed or was already gone.
   */
  async revokeToken(workspaceId: string, username: string): Promise<void> {
    await apiClient.delete(`/airhouse/me/tokens/${encodeURIComponent(username)}`, {
      params: { workspace_id: workspaceId }
    });
  },

  /**
   * Current state of the workspace tenant's DuckLake catalog hot-path indexes.
   * Owner/Admin only. 404 when the workspace has no provisioned tenant.
   */
  async getCatalogIndexes(workspaceId: string): Promise<CatalogIndexesStatus> {
    const response = await apiClient.get("/airhouse/me/catalog-indexes", {
      params: { workspace_id: workspaceId }
    });
    return response.data;
  },

  /**
   * Toggle the catalog hot-path indexes on/off. Owner/Admin only. Airhouse
   * builds/drops them asynchronously (CONCURRENTLY) — `accepted` just means it
   * took the request; re-fetch `getCatalogIndexes` to watch `state` settle to
   * `"ready"`.
   */
  async setCatalogIndexes(
    workspaceId: string,
    enabled: boolean
  ): Promise<CatalogIndexesToggleResult> {
    const response = await apiClient.put(
      "/airhouse/me/catalog-indexes",
      { enabled },
      { params: { workspace_id: workspaceId } }
    );
    return response.data;
  }
};
