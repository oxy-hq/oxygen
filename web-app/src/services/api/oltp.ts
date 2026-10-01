import { apiClient } from "./axios";

/** A writer's schema inside the per-org OLTP database. */
interface OltpSchemaInfo {
  schema: string;
  /** `app` (a custom app) or `pipeline` (an Airway pipeline). */
  kind: "app" | "pipeline";
  writer_name: string;
  role: string;
  /** Whether the read-only analyst can read this schema. */
  analytics_visible: boolean;
}

export interface OltpConnectionInfo {
  is_provisioned: boolean;
  host: string;
  database: string;
  provider: string;
  /** The provider's name for this project (`oxy-org-<uuid>`). */
  project_name: string;
  /** A clickable console link (Neon only; `null` on local/mock). */
  console_url: string | null;
  region: string;
  status: string;
  /** The role every human and agent query resolves to. Read-only, always. */
  analyst_role: string;
  /** Without this, `postgres_managed` cannot resolve. */
  analyst_ready: boolean;
  platform_schema_version: number;
  expected_platform_schema_version: number;
  schemas: OltpSchemaInfo[];
}

export const OltpService = {
  /**
   * Status of the caller's org OLTP database.
   *
   * Returns **no credentials** — deliberately, and unlike the Airhouse
   * equivalent. A per-org OLTP database holds live business records; queries
   * go through the IDE via `type: postgres_managed`, which resolves the
   * read-only analyst server-side.
   */
  async getConnection(workspaceId: string): Promise<OltpConnectionInfo> {
    const response = await apiClient.get("/oltp/me/connection", {
      params: { workspace_id: workspaceId }
    });
    // The SPA catch-all answers unknown paths with index.html and HTTP 200, so
    // a missing route reaches callers as a successful response whose body is a
    // string. Reading `is_provisioned` off that yields `undefined`, which reads
    // as "not provisioned yet" — a stale server then looks exactly like an
    // unprovisioned org. Fail loudly instead.
    const data: unknown = response.data;
    if (typeof data !== "object" || data === null || !("is_provisioned" in data)) {
      throw new Error(
        "GET /oltp/me/connection did not return connection info. The server is " +
          "probably running a build without this route — restart it."
      );
    }
    return data as OltpConnectionInfo;
  }
};

/** A column, as the diagram draws it. */
interface ErdColumn {
  name: string;
  data_type: string;
  nullable: boolean;
  is_primary_key: boolean;
}

export interface ErdTable {
  name: string;
  columns: ErdColumn[];
}

export interface ErdSchema {
  name: string;
  /** `app`, `pipeline`, or `other` when no writer owns it. */
  kind: "app" | "pipeline" | "other";
  writer_name: string | null;
  tables: ErdTable[];
}

/** A foreign key, as an edge between two columns. */
interface ErdRelationship {
  from_schema: string;
  from_table: string;
  from_column: string;
  to_schema: string;
  to_table: string;
  to_column: string;
}

export interface ErdResponse {
  database: string;
  schemas: ErdSchema[];
  relationships: ErdRelationship[];
  /** Always the analyst — the diagram is structure, never rows. */
  read_as_role: string;
}

/**
 * Structure of the org's OLTP database: schemas, tables, columns, foreign keys.
 * Returns no row data — reading it runs as the read-only analyst.
 */
export async function getOltpErd(workspaceId: string): Promise<ErdResponse> {
  const response = await apiClient.get("/oltp/me/erd", {
    params: { workspace_id: workspaceId }
  });
  return response.data;
}

// ── Admin (staff / partner) ────────────────────────────────────────────────
//
// Org-keyed, not workspace-keyed: an operator provisioning for a tenant is not
// a member of any workspace in it, so `/oltp/me/*` cannot answer for them.

export interface OltpCredentials {
  role: string;
  dsn: string;
  /** Whether this DSN can write. The UI leads with it. */
  writable: boolean;
}

export const AdminOltpService = {
  /** Status for any org. Same shape as the member view, same server code. */
  async getStatus(orgId: string): Promise<OltpConnectionInfo> {
    const response = await apiClient.get(`/admin/orgs/${orgId}/oltp`);
    const data: unknown = response.data;
    // Same SPA-catch-all guard as the member endpoint: an unknown path answers
    // index.html with HTTP 200, and reading `is_provisioned` off a string gives
    // `undefined` — which renders as "not provisioned" and makes a stale server
    // look exactly like an org that needs provisioning.
    if (typeof data !== "object" || data === null || !("is_provisioned" in data)) {
      throw new Error(
        `GET /admin/orgs/${orgId}/oltp did not return status. The server is probably ` +
          "running a build without this route — restart it."
      );
    }
    return data as OltpConnectionInfo;
  },

  /** Idempotent — a double-click converges on one database, not two. */
  async provision(orgId: string, writers: string[] = []): Promise<OltpConnectionInfo> {
    const response = await apiClient.post(`/admin/orgs/${orgId}/oltp/provision`, { writers });
    return response.data as OltpConnectionInfo;
  },

  /** Let the read-only analyst read a writer's schema, or withdraw it. */
  async setVisibility(
    orgId: string,
    writer: string,
    visible: boolean
  ): Promise<OltpConnectionInfo> {
    const response = await apiClient.post(`/admin/orgs/${orgId}/oltp/visibility`, {
      writer,
      visible
    });
    return response.data as OltpConnectionInfo;
  },

  /** Destroy the provider database. Irreversible. */
  async deprovision(orgId: string): Promise<OltpConnectionInfo> {
    const response = await apiClient.delete(`/admin/orgs/${orgId}/oltp`);
    return response.data as OltpConnectionInfo;
  },

  /**
   * POST, not GET: disclosing a live credential is an event, not a read. It
   * must not land in browser history or a proxy log, and the server records it.
   *
   * @param role `analyst` for the read-only DSN, or `app:<slug>` / `pipeline:<src>`
   *             for a writable one.
   */
  async credentials(orgId: string, role: string): Promise<OltpCredentials> {
    const response = await apiClient.post(`/admin/orgs/${orgId}/oltp/credentials`, { role });
    return response.data as OltpCredentials;
  }
};

/** One org's OLTP database in the fleet-wide list. */
export interface OltpTenantRow {
  org_id: string;
  org_name: string;
  database: string;
  host: string;
  provider: string;
  region: string;
  /** `none` when the org has no database yet. */
  status: string;
  /**
   * The schemas themselves, not a count. The console renders them as chips —
   * "2" could not say whether analytics reads the app's live rows, which is
   * the fact an operator scans for and fits in the same width.
   */
  schemas: OltpTenantSchema[];
  analyst_ready: boolean;
  platform_drift: boolean;
}

/** A schema as the fleet list sees it: what it is, not how to connect to it. */
interface OltpTenantSchema {
  schema: string;
  kind: "app" | "pipeline";
  analytics_visible: boolean;
}

/** Every org, provisioned or not — "who still needs one" is the usual question. */
export async function listOltpTenants(): Promise<OltpTenantRow[]> {
  const response = await apiClient.get("/admin/oltp");
  const data: unknown = response.data;
  if (!Array.isArray(data)) {
    throw new Error(
      "GET /admin/oltp did not return a list. The server is probably running a " +
        "build without this route — restart it."
    );
  }
  return data as OltpTenantRow[];
}
