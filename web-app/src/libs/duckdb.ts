import * as duckdb from "@duckdb/duckdb-wasm";
import { encodeBase64 } from "@/libs/encoding";

enum InitState {
  Uninitialized,
  Initializing,
  Initialized
}

let duckDB: duckdb.AsyncDuckDB = null!;
let initPromise: Promise<void> | null = null;
let initState = InitState.Uninitialized;

const init = async () => {
  if (initState === InitState.Initialized) return;
  if (!initPromise) {
    initState = InitState.Initializing;
    initPromise = (async () => {
      try {
        console.debug("Initializing DuckDB");
        let bundle: duckdb.DuckDBBundle;
        let worker: Worker;
        // Dev server: the bundles Vite serves out of node_modules, so a dev
        // loop needs no network and gets a wasm that matches the installed
        // `@duckdb/duckdb-wasm` exactly.
        //
        // Every build but the dev server's takes the jsDelivr path below —
        // including a production binary on `localhost` (`oxy start`,
        // `oxy serve`, the CLI release). This used to branch on
        // `window.location.hostname`, which made one binary behave two ways
        // depending on the host it answered on: exactly the dev-vs-prod
        // heuristic CLAUDE.md rules out, and the reason 71.7 MiB of wasm was
        // embedded in every binary to serve a case that only arises on a
        // laptop. Self-hosted installs on a real domain have always loaded
        // DuckDB from jsDelivr; local runs now do the same, so a run with no
        // route to jsDelivr can no longer render a result table.
        if (import.meta.env.DEV) {
          const { localBundles } = await import("@/libs/duckdb-local-bundles");
          bundle = await duckdb.selectBundle(localBundles);
          worker = new Worker(bundle.mainWorker!, { type: "module" });
        } else {
          // Use CDN bundles for cloud
          const JSDELIVR_BUNDLES = duckdb.getJsDelivrBundles();
          bundle = await duckdb.selectBundle(JSDELIVR_BUNDLES);
          const worker_url = URL.createObjectURL(
            new Blob([`importScripts("${bundle.mainWorker!}");`], {
              type: "text/javascript"
            })
          );
          worker = new Worker(worker_url);
          // Use a local variable so `duckDB` is only assigned after instantiation
          // completes. Assigning it earlier (as a side-effect inside the call
          // arguments) caused a race: a concurrent getDuckDB() would see a truthy
          // `duckDB`, skip `await init()`, and try to connect to an uninstantiated
          // instance, triggering "DuckDB not initialized" errors.
          const cdnDb = new duckdb.AsyncDuckDB(new duckdb.ConsoleLogger(), worker);
          await cdnDb.instantiate(bundle.mainModule, bundle.pthreadWorker);
          duckDB = cdnDb;
          URL.revokeObjectURL(worker_url);
          initState = InitState.Initialized;
          return;
        }
        const logger = new duckdb.ConsoleLogger();
        const localDb = new duckdb.AsyncDuckDB(logger, worker);
        await localDb.instantiate(bundle.mainModule, bundle.pthreadWorker);
        duckDB = localDb;
        initState = InitState.Initialized;
      } catch (e) {
        initState = InitState.Uninitialized;
        initPromise = null;
        throw e;
      }
    })();
  }
  return initPromise;
};

export const getDuckDB = async () => {
  if (!duckDB) {
    await init();
  }
  return duckDB;
};

// Registrations in flight, keyed by the table name a `filePath` deterministically
// maps to (same file → same key). `registerAuthenticatedParquetFile` runs a
// DROP-then-CREATE pair against that table; two overlapping calls for the SAME
// file — a React 19 dev-mode StrictMode double-invoked effect, a rapid re-render,
// a fast double-click on Run — used to race each other: both DROPs could land
// before either CREATE, so the second CREATE hit "already exists" even though
// nothing was actually wrong. Dedupe on the promise itself (same pattern as
// `initPromise` above) so a second caller for the same file awaits the first
// call's result instead of starting a competing DROP/CREATE pair.
const registrationsInFlight = new Map<string, Promise<string>>();

/**
 * Register a Parquet file from the API endpoint with authentication
 * @param filePath - The file path to register (e.g., result file path from API)
 * @param projectId - The project ID for authentication
 * @param branchName - The branch name for the request
 * @returns The registered table name in DuckDB
 */
export const registerAuthenticatedParquetFile = async (
  filePath: string,
  projectId: string,
  branchName: string
): Promise<string> => {
  // Table name is a pure function of filePath, so it's also the dedupe key —
  // two calls that would create/race the same table are, by construction, the
  // same logical request.
  const tableName = encodeBase64(filePath).replace(/[^a-zA-Z0-9]/g, "_");
  const inFlight = registrationsInFlight.get(tableName);
  if (inFlight) return inFlight;

  const promise = registerAuthenticatedParquetFileUncached(
    filePath,
    projectId,
    branchName,
    tableName
  );
  registrationsInFlight.set(tableName, promise);
  try {
    return await promise;
  } finally {
    // Only this call's own promise clears the slot — a call that arrived
    // while this one was in flight already resolved off the same promise
    // and has nothing left to clear.
    if (registrationsInFlight.get(tableName) === promise) {
      registrationsInFlight.delete(tableName);
    }
  }
};

const registerAuthenticatedParquetFileUncached = async (
  filePath: string,
  projectId: string,
  branchName: string,
  tableName: string
): Promise<string> => {
  const db = await getDuckDB();

  const { apiClient } = await import("@/services/api/axios");

  const response = await apiClient.get(`/${projectId}/results/files/${filePath}`, {
    responseType: "arraybuffer",
    params: { branch: branchName }
  });

  const fileData = new Uint8Array(response.data);

  const conn = await db.connect();

  // Drop table if it exists to ensure fresh data
  try {
    await conn.query(`DROP TABLE IF EXISTS "${tableName}"`);
  } catch (e) {
    console.warn("Error dropping table:", e);
  }

  // Insert Parquet data directly into DuckDB
  try {
    // Register the Parquet data as a file in DuckDB's virtual filesystem
    await db.registerFileBuffer(`${tableName}.parquet`, fileData);

    // Phase 1: CREATE TABLE as a DDL statement — DuckDB returns a row-count
    // result (a single INTEGER), never materializing any actual column data.
    // This avoids Arrow Int64 → JS BigInt conversion for large BIGINT values
    // that would throw "is not safe to convert to a number".
    await conn.query(
      `CREATE TABLE "${tableName}" AS SELECT * FROM parquet_scan('${tableName}.parquet')`
    );

    // Phase 2: Query information_schema.columns — all result columns are
    // VARCHAR/INTEGER, completely safe from BigInt overflow.
    const escapedName = tableName.replace(/'/g, "''");
    const colTypesResult = await conn.query(
      `SELECT column_name, data_type
       FROM information_schema.columns
       WHERE table_name = '${escapedName}' AND table_schema = 'main'
       ORDER BY ordinal_position`
    );
    const colTypes = colTypesResult.toArray() as Array<{
      column_name: string;
      data_type: string;
    }>;

    // Phase 3: If any BIGINT-family columns exist, recreate the table with
    // those columns cast to VARCHAR so JS never sees an unsafe Int64 value.
    const bigintTypes = new Set(["BIGINT", "UBIGINT", "HUGEINT", "INT8"]);
    const hasBigInt = colTypes.some(({ data_type }) => bigintTypes.has(data_type));
    if (hasBigInt) {
      const selectExprs = colTypes.map(({ column_name, data_type }) => {
        const escaped = column_name.replace(/"/g, '""');
        return bigintTypes.has(data_type)
          ? `"${escaped}"::VARCHAR AS "${escaped}"`
          : `"${escaped}"`;
      });
      await conn.query(`DROP TABLE "${tableName}"`);
      await conn.query(
        `CREATE TABLE "${tableName}" AS SELECT ${selectExprs.join(", ")} FROM parquet_scan('${tableName}.parquet')`
      );
    }

    // Verify the table was created
    await conn.query(`SELECT COUNT(*) as cnt FROM "${tableName}"`);
  } catch (e) {
    console.error("Error loading Parquet data:", e);
    await conn.close();
    throw e;
  }

  await conn.close();
  return tableName;
};
