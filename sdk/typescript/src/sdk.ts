import type { AnomaliesClient } from "./anomalies";
import { OxyClient } from "./client";
import { createConfigAsync, type OxyConfig } from "./config";
import type { MetricTreeClient } from "./metricTree";
import { ParquetReader, type QueryResult } from "./parquet";

/**
 * OxySDK provides a unified interface for fetching data from Oxy and querying it with SQL.
 * It combines OxyClient (for API calls) and ParquetReader (for SQL queries) into a single,
 * easy-to-use interface.
 *
 * @example
 * ```typescript
 * // Create SDK instance
 * const sdk = new OxySDK({ apiKey: 'your-key', projectId: 'your-project' });
 *
 * // Register a parquet blob and query it
 * await sdk.getReader().registerParquet(salesBlob, 'sales');
 * const result = await sdk.query('SELECT * FROM sales WHERE amount > 1000');
 * console.log(result.rows);
 *
 * // Clean up when done
 * await sdk.close();
 * ```
 */
export class OxySDK {
  private client: OxyClient;
  private reader: ParquetReader;

  constructor(config: OxyConfig) {
    this.client = new OxyClient(config);
    this.reader = new ParquetReader();
  }

  /**
   * Creates an OxySDK instance asynchronously with support for postMessage authentication
   *
   * @param config - Optional configuration overrides
   * @returns Promise resolving to OxySDK instance
   *
   * @example
   * ```typescript
   * // In an iframe - automatic postMessage auth
   * const sdk = await OxySDK.create({
   *   parentOrigin: 'https://app.example.com',
   *   projectId: 'my-project-id'
   * });
   * ```
   */
  static async create(config?: Partial<OxyConfig>): Promise<OxySDK> {
    const resolvedConfig = await createConfigAsync(config);
    return new OxySDK(resolvedConfig);
  }

  /**
   * Execute a SQL query against loaded data
   *
   * @param sql - SQL query to execute
   * @returns Query result with columns and rows
   *
   * @example
   * ```typescript
   * await sdk.getReader().registerParquet(salesBlob, 'sales');
   *
   * const result = await sdk.query('SELECT product, SUM(amount) as total FROM sales GROUP BY product');
   * console.log(result.columns); // ['product', 'total']
   * console.log(result.rows);    // [['Product A', 1000], ['Product B', 2000]]
   * console.log(result.rowCount); // 2
   * ```
   */
  async query(sql: string): Promise<QueryResult> {
    return this.reader.query(sql);
  }

  /**
   * Get all data from a loaded table
   *
   * @param tableName - Name of the table
   * @param limit - Maximum number of rows (optional)
   * @returns Query result
   *
   * @example
   * ```typescript
   * await sdk.getReader().registerParquet(salesBlob, 'sales');
   * const allData = await sdk.getAll('sales');
   * const first100 = await sdk.getAll('sales', 100);
   * ```
   */
  async getAll(tableName: string, limit?: number): Promise<QueryResult> {
    return this.reader.getAll(tableName, limit);
  }

  /**
   * Get schema information for a loaded table
   *
   * @param tableName - Name of the table
   * @returns Schema information
   *
   * @example
   * ```typescript
   * await sdk.getReader().registerParquet(salesBlob, 'sales');
   * const schema = await sdk.getSchema('sales');
   * console.log(schema.columns); // ['column_name', 'column_type', ...]
   * console.log(schema.rows);    // [['id', 'INTEGER'], ['name', 'VARCHAR'], ...]
   * ```
   */
  async getSchema(tableName: string): Promise<QueryResult> {
    return this.reader.getSchema(tableName);
  }

  /**
   * Get row count for a loaded table
   *
   * @param tableName - Name of the table
   * @returns Number of rows
   *
   * @example
   * ```typescript
   * await sdk.getReader().registerParquet(salesBlob, 'sales');
   * const count = await sdk.count('sales');
   * console.log(`Total rows: ${count}`);
   * ```
   */
  async count(tableName: string): Promise<number> {
    return this.reader.count(tableName);
  }

  /**
   * Metric-tree analyses (tree introspection, sensitivity, predict, explain,
   * opportunity). Pass-through to the underlying `OxyClient.metricTree`.
   *
   * @example
   * ```typescript
   * const tree = await sdk.metricTree.getTree();
   * const sensitivity = await sdk.metricTree.getSensitivity("orders.net_revenue");
   * ```
   */
  get metricTree(): MetricTreeClient {
    return this.client.metricTree;
  }

  /**
   * Anomaly inbox (list / scan / update status / explain). Pass-through to
   * the underlying `OxyClient.anomalies`.
   *
   * @example
   * ```typescript
   * await sdk.anomalies.scan({ as_of: "2025-12-15" });
   * const { anomalies } = await sdk.anomalies.list({ status: "new" });
   * ```
   */
  get anomalies(): AnomaliesClient {
    return this.client.anomalies;
  }

  /**
   * Get direct access to the underlying OxyClient
   *
   * Useful for the metric-tree, anomaly and peer-cohort clients.
   *
   * @returns The OxyClient instance
   *
   * @example
   * ```typescript
   * const tree = await sdk.getClient().metricTree.getTree();
   * ```
   */
  getClient(): OxyClient {
    return this.client;
  }

  /**
   * Get direct access to the underlying ParquetReader
   *
   * Useful for advanced operations like registering blobs directly.
   *
   * @returns The ParquetReader instance
   *
   * @example
   * ```typescript
   * const myBlob = new Blob([parquetData]);
   * await sdk.getReader().registerParquet(myBlob, 'mydata');
   * ```
   */
  getReader(): ParquetReader {
    return this.reader;
  }

  /**
   * Close and cleanup all resources
   *
   * This clears all loaded data and releases resources. Call this when you're done with the SDK.
   *
   * @example
   * ```typescript
   * const sdk = new OxySDK({ apiKey: 'key', projectId: 'project' });
   * await sdk.getReader().registerParquet(salesBlob, 'sales');
   * const result = await sdk.query('SELECT * FROM sales');
   * await sdk.close(); // Clean up
   * ```
   */
  async close(): Promise<void> {
    await this.reader.close();
  }
}
