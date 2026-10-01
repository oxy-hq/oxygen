import type {
  ConnectionTestEvent,
  CreateDatabaseConfigResponse,
  DatabaseInfo,
  DatabaseSchema,
  DatabaseSyncResponse,
  InspectEvent,
  SchemaListResult,
  SchemaTablesResult,
  TestDatabaseConnectionRequest,
  WarehousesFormData
} from "@/types/database";
import { apiClient } from "./axios";
import fetchSSE from "./fetchSSE";

// Response can be either:
// - string[][] (default JSON format) - the result array directly
// - Parquet metadata (when result_format is "parquet") - a file handle plus
//   `truncated`, which is `true` when the result hit the ad-hoc 10k row cap.
type ExecuteSqlParquetResponse = {
  file_name: string;
  is_preagg?: boolean;
  execution_time_ms?: number;
  truncated?: boolean;
};
export type ExecuteSqlResponse = string[][] | ExecuteSqlParquetResponse;

export class DatabaseService {
  static async getDatabaseSchema(
    projectId: string,
    branchName: string,
    dbName: string
  ): Promise<DatabaseSchema> {
    const response = await apiClient.get(
      `/${projectId}/databases/${encodeURIComponent(dbName)}/schema`,
      { params: { branch: branchName } }
    );
    return response.data;
  }

  static async listDatabases(projectId: string, branchName: string): Promise<DatabaseInfo[]> {
    const response = await apiClient.get(`/${projectId}/databases`, {
      params: { branch: branchName }
    });
    return response.data;
  }

  static async executeSql(
    projectId: string,
    branchName: string,
    pathb64: string,
    sql: string,
    database: string
  ): Promise<ExecuteSqlResponse> {
    const response = await apiClient.post(
      `/${projectId}/sql/${pathb64}`,
      {
        sql,
        database,
        result_format: "parquet"
      },
      { params: { branch: branchName } }
    );
    return response.data;
  }

  static async executeSqlQuery(
    projectId: string,
    branchName: string,
    sql: string,
    database: string
  ): Promise<ExecuteSqlResponse> {
    const response = await apiClient.post(
      `/${projectId}/sql/query`,
      {
        sql,
        database,
        result_format: "parquet"
      },
      { params: { branch: branchName } }
    );
    return response.data;
  }

  static async syncDatabase(
    projectId: string,
    branchName: string,
    database?: string,
    options?: { datasets?: string[]; tables?: string[] }
  ): Promise<DatabaseSyncResponse> {
    const params = new URLSearchParams();
    params.append("branch", branchName);
    if (database) params.append("database", database);
    if (options?.datasets && options.datasets.length > 0) {
      for (const dataset of options.datasets) {
        params.append("datasets", dataset);
      }
    }
    if (options?.tables && options.tables.length > 0) {
      params.append("tables", options.tables.join(","));
    }

    const response = await apiClient.post(`/${projectId}/databases/sync?${params.toString()}`);
    return response.data;
  }

  static async buildDatabase(
    projectId: string,
    branchName: string
  ): Promise<{
    success: boolean;
    message?: string;
  }> {
    const response = await apiClient.post(`/${projectId}/databases/build`, {
      params: { branch: branchName }
    });
    return response.data;
  }

  static async cleanData(
    projectId: string,
    branchName: string,
    target?: string
  ): Promise<{
    success: boolean;
    message: string;
    cleaned_items: string[];
  }> {
    const params = new URLSearchParams();
    params.append("branch", branchName);
    if (target) params.append("target", target);

    const response = await apiClient.post(`/${projectId}/databases/clean?${params.toString()}`);
    return response.data;
  }

  static async createDatabaseConfig(
    projectId: string,
    branchName: string,
    warehouses: WarehousesFormData
  ): Promise<CreateDatabaseConfigResponse> {
    const response = await apiClient.post(`/${projectId}/databases`, warehouses, {
      params: { branch: branchName }
    });
    return response.data;
  }

  /**
   * Test database connection with SSE streaming for real-time updates
   * @param projectId - The project ID
   * @param branchName - The branch name
   * @param request - Test connection request with warehouse config
   * @param onEvent - Callback to handle streaming events (progress, browser auth, completion)
   * @returns Promise that resolves when the connection test completes
   */
  static async testDatabaseConnection(
    projectId: string,
    branchName: string,
    request: TestDatabaseConnectionRequest,
    onEvent: (event: ConnectionTestEvent) => void
  ): Promise<void> {
    const baseUrl = apiClient.defaults.baseURL || "";
    const url = `${baseUrl}/${projectId}/databases/test-connection?branch=${encodeURIComponent(branchName)}`;

    await fetchSSE<ConnectionTestEvent>(url, {
      method: "POST",
      body: request,
      onMessage: onEvent
    });
  }

  /**
   * Lightweight schema/table discovery for the onboarding table picker.
   * Returns just `{ schema, table, column_count }` per table. Full column
   * metadata is pulled lazily by `syncDatabase({ tables: [...] })` once the
   * user has selected which tables to include.
   */
  static async inspectDatabase(
    projectId: string,
    branchName: string,
    database: string | undefined,
    onEvent: (event: InspectEvent) => void
  ): Promise<void> {
    const params = new URLSearchParams();
    params.append("branch", branchName);
    if (database) params.append("database", database);
    const baseUrl = apiClient.defaults.baseURL || "";
    const url = `${baseUrl}/${projectId}/databases/inspect?${params.toString()}`;

    await fetchSSE<InspectEvent>(url, {
      method: "POST",
      onMessage: onEvent
    });
  }

  /**
   * Fast schema-only discovery: returns schema names + total table counts
   * per schema via a single INFORMATION_SCHEMA.TABLES scan. Tables for each
   * schema are fetched lazily via `inspectSchemaTables` when the user expands
   * a schema in the picker.
   */
  static async inspectSchemas(
    projectId: string,
    branchName: string,
    database?: string
  ): Promise<SchemaListResult> {
    const params = new URLSearchParams();
    params.append("branch", branchName);
    if (database) params.append("database", database);
    const response = await apiClient.post<SchemaListResult>(
      `/${projectId}/databases/inspect-schemas?${params.toString()}`
    );
    return response.data;
  }

  /**
   * Lazy per-schema table listing. One cheap GROUP BY query against the
   * schema's columns metadata — called when the user expands a schema.
   */
  static async inspectSchemaTables(
    projectId: string,
    branchName: string,
    schema: string,
    database?: string
  ): Promise<SchemaTablesResult> {
    const params = new URLSearchParams();
    params.append("branch", branchName);
    params.append("schema", schema);
    if (database) params.append("database", database);
    const response = await apiClient.post<SchemaTablesResult>(
      `/${projectId}/databases/inspect-schema-tables?${params.toString()}`
    );
    return response.data;
  }
}
