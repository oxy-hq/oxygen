import * as duckdb from "@duckdb/duckdb-wasm";

let dbInstance: duckdb.AsyncDuckDB | null = null;
let connection: duckdb.AsyncDuckDBConnection | null = null;

// Queue to serialize operations and prevent race conditions
let operationQueue = Promise.resolve();

/**
 * Enqueue an operation to prevent race conditions on shared DuckDB instance
 */
function enqueueOperation<T>(operation: () => Promise<T>): Promise<T> {
  const currentOperation = operationQueue.then(operation, operation);
  operationQueue = currentOperation.then(
    () => {
      return;
    },
    () => {
      return;
    }
  );
  return currentOperation;
}

/**
 * Initialize DuckDB-WASM instance
 */
export async function initializeDuckDB(): Promise<duckdb.AsyncDuckDB> {
  if (dbInstance) {
    return dbInstance;
  }

  const JSDELIVR_BUNDLES = duckdb.getJsDelivrBundles();

  // Select a bundle based on browser features
  const bundle = await duckdb.selectBundle(JSDELIVR_BUNDLES);

  const worker_url = URL.createObjectURL(
    new Blob([`importScripts("${bundle.mainWorker}");`], {
      type: "text/javascript"
    })
  );

  const worker = new Worker(worker_url);
  const logger = new duckdb.ConsoleLogger();

  dbInstance = new duckdb.AsyncDuckDB(logger, worker);
  await dbInstance.instantiate(bundle.mainModule, bundle.pthreadWorker);
  URL.revokeObjectURL(worker_url);

  return dbInstance;
}

/**
 * Get or create a DuckDB connection
 */
async function getConnection(): Promise<duckdb.AsyncDuckDBConnection> {
  if (connection) {
    return connection;
  }

  const db = await initializeDuckDB();
  connection = await db.connect();
  return connection;
}

/**
 * Query result interface
 */
export interface QueryResult {
  columns: string[];
  rows: unknown[][];
  rowCount: number;
}

/**
 * ParquetReader provides methods to read and query Parquet files.
 * Supports registering multiple Parquet files with different table names.
 */
export class ParquetReader {
  private tableMap: Map<string, string> = new Map(); // Maps user table name -> internal unique table name

  /**
   * Generate a unique internal table name to prevent conflicts
   */
  private generateInternalTableName(tableName: string): string {
    // Note: Math.random() is acceptable here as uniqueness is only needed for table naming (not security-critical)
    /* eslint-disable sonarjs/pseudo-random */
    const uniqueId = `${Date.now()}_${Math.random().toString(36).substring(2, 9)}`;
    /* eslint-enable sonarjs/pseudo-random */
    return `${tableName}_${uniqueId}`;
  }

  /**
   * Register a Parquet file from a Blob with a specific table name
   *
   * @param blob - Parquet file as Blob
   * @param tableName - Name to use for the table in queries (required)
   *
   * @example
   * ```typescript
   * const blob = await (await fetch(salesParquetUrl)).blob();
   * const reader = new ParquetReader();
   * await reader.registerParquet(blob, 'sales');
   * ```
   *
   * @example
   * ```typescript
   * // Register multiple files
   * const reader = new ParquetReader();
   * await reader.registerParquet(salesBlob, 'sales');
   * await reader.registerParquet(customersBlob, 'customers');
   * const result = await reader.query('SELECT * FROM sales JOIN customers ON sales.customer_id = customers.id');
   * ```
   */
  async registerParquet(blob: Blob, tableName: string): Promise<void> {
    const internalTableName = this.generateInternalTableName(tableName);

    await enqueueOperation(async () => {
      const conn = await getConnection();
      const db = await initializeDuckDB();

      const arrayBuffer = await blob.arrayBuffer();
      const uint8Array = new Uint8Array(arrayBuffer);

      // Register the file with DuckDB using unique name
      await db.registerFileBuffer(`${internalTableName}.parquet`, uint8Array);

      try {
        await conn.query(`DROP TABLE IF EXISTS ${internalTableName}`);
      } catch {
        // Ignore error if table doesn't exist
      }

      await conn.query(
        `CREATE TABLE ${internalTableName} AS SELECT * FROM '${internalTableName}.parquet'`
      );

      // Store mapping
      this.tableMap.set(tableName, internalTableName);
    });
  }

  /**
   * Register multiple Parquet files at once
   *
   * @param files - Array of objects containing blob and tableName
   *
   * @example
   * ```typescript
   * const reader = new ParquetReader();
   * await reader.registerMultipleParquet([
   *   { blob: salesBlob, tableName: 'sales' },
   *   { blob: customersBlob, tableName: 'customers' },
   *   { blob: productsBlob, tableName: 'products' }
   * ]);
   * const result = await reader.query('SELECT * FROM sales JOIN customers ON sales.customer_id = customers.id');
   * ```
   */
  async registerMultipleParquet(files: Array<{ blob: Blob; tableName: string }>): Promise<void> {
    for (const file of files) {
      await this.registerParquet(file.blob, file.tableName);
    }
  }

  /**
   * Execute a SQL query against the registered Parquet data
   *
   * @param sql - SQL query string
   * @returns Query result with columns and rows
   *
   * @example
   * ```typescript
   * const result = await reader.query('SELECT * FROM sales LIMIT 10');
   * console.log(result.columns);
   * console.log(result.rows);
   * ```
   *
   * @example
   * ```typescript
   * // Query multiple tables
   * await reader.registerParquet(salesBlob, 'sales');
   * await reader.registerParquet(customersBlob, 'customers');
   * const result = await reader.query(`
   *   SELECT s.*, c.name
   *   FROM sales s
   *   JOIN customers c ON s.customer_id = c.id
   * `);
   * ```
   */
  async query(sql: string): Promise<QueryResult> {
    if (this.tableMap.size === 0) {
      throw new Error("No Parquet files registered. Call registerParquet() first.");
    }

    return enqueueOperation(async () => {
      const conn = await getConnection();

      // Replace all registered table names with their internal unique names
      let rewrittenSql = sql;
      for (const [userTableName, internalTableName] of this.tableMap.entries()) {
        rewrittenSql = rewrittenSql.replace(
          new RegExp(`\\b${userTableName}\\b`, "g"),
          internalTableName
        );
      }

      const result = await conn.query(rewrittenSql);

      const columns = result.schema.fields.map((field) => field.name);
      const rows: unknown[][] = [];

      // Convert Arrow table to rows
      for (let i = 0; i < result.numRows; i++) {
        const row: unknown[] = [];
        for (let j = 0; j < result.numCols; j++) {
          const col = result.getChildAt(j);
          row.push(col?.get(i));
        }
        rows.push(row);
      }

      return {
        columns,
        rows,
        rowCount: result.numRows
      };
    });
  }

  /**
   * Get all data from a registered table
   *
   * @param tableName - Name of the table to query
   * @param limit - Maximum number of rows to return (default: all)
   * @returns Query result
   *
   * @example
   * ```typescript
   * const allData = await reader.getAll('sales');
   * const first100 = await reader.getAll('sales', 100);
   * ```
   */
  async getAll(tableName: string, limit?: number): Promise<QueryResult> {
    const limitClause = limit ? ` LIMIT ${limit}` : "";
    return this.query(`SELECT * FROM ${tableName}${limitClause}`);
  }

  /**
   * Get table schema information
   *
   * @param tableName - Name of the table to describe
   * @returns Schema information
   *
   * @example
   * ```typescript
   * const schema = await reader.getSchema('sales');
   * console.log(schema.columns); // ['id', 'name', 'sales']
   * console.log(schema.rows);    // [['id', 'INTEGER'], ['name', 'VARCHAR'], ...]
   * ```
   */
  async getSchema(tableName: string): Promise<QueryResult> {
    return this.query(`DESCRIBE ${tableName}`);
  }

  /**
   * Get row count for a table
   *
   * @param tableName - Name of the table to count
   * @returns Number of rows in the table
   *
   * @example
   * ```typescript
   * const count = await reader.count('sales');
   * console.log(`Total rows: ${count}`);
   * ```
   */
  async count(tableName: string): Promise<number> {
    const result = await this.query(`SELECT COUNT(*) as count FROM ${tableName}`);
    return result.rows[0][0] as number;
  }

  /**
   * Close and cleanup all registered resources
   */
  async close(): Promise<void> {
    if (this.tableMap.size > 0) {
      await enqueueOperation(async () => {
        const conn = await getConnection();
        const db = await initializeDuckDB();

        // Drop all registered tables and file buffers
        for (const [, internalTableName] of this.tableMap.entries()) {
          try {
            await conn.query(`DROP TABLE IF EXISTS ${internalTableName}`);
          } catch {
            // Ignore error
          }

          // Drop the registered file buffer using internal unique name
          try {
            await db.dropFile(`${internalTableName}.parquet`);
          } catch {
            // Ignore error if file doesn't exist
          }
        }

        this.tableMap.clear();
      });
    }
  }
}

/**
 * Helper function to quickly read a Parquet blob and execute a query
 *
 * @param blob - Parquet file as Blob
 * @param tableName - Name to use for the table in queries (default: 'data')
 * @param sql - SQL query to execute (optional, defaults to SELECT * FROM tableName)
 * @returns Query result
 *
 * @example
 * ```typescript
 * const blob = await (await fetch(salesParquetUrl)).blob();
 * const result = await queryParquet(blob, 'sales', 'SELECT product, SUM(amount) as total FROM sales GROUP BY product');
 * console.log(result);
 * ```
 */
export async function queryParquet(
  blob: Blob,
  tableName: string = "data",
  sql?: string
): Promise<QueryResult> {
  const reader = new ParquetReader();

  await reader.registerParquet(blob, tableName);

  const query = sql || `SELECT * FROM ${tableName}`;
  const result = await reader.query(query);

  await reader.close();
  return result;
}

/**
 * Helper function to read Parquet file and get all data
 *
 * @param blob - Parquet file as Blob
 * @param tableName - Name to use for the table (default: 'data')
 * @param limit - Maximum number of rows (optional)
 * @returns Query result
 *
 * @example
 * ```typescript
 * const blob = await (await fetch(salesParquetUrl)).blob();
 * const data = await readParquet(blob, 'sales', 1000);
 * console.log(`Loaded ${data.rowCount} rows`);
 * ```
 */
export async function readParquet(
  blob: Blob,
  tableName: string = "data",
  limit?: number
): Promise<QueryResult> {
  const reader = new ParquetReader();

  await reader.registerParquet(blob, tableName);

  const result = await reader.getAll(tableName, limit);

  await reader.close();
  return result;
}
