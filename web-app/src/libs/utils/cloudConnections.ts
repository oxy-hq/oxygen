/**
 * Which database connections the web app offers to create, by deployment mode.
 *
 * In a cloud deployment no pod has a disk that is the workspace, so nothing the
 * app creates may resolve against one: no file DuckDB, no BigQuery key file
 * (`key_path`), no Snowflake private-key file (`private_key_path`). A key's
 * contents go in the workspace's secret store instead, and the config names
 * the secret — `key_json_var` for BigQuery, `private_key_var` for Snowflake.
 * Uploaded files are loaded into the workspace's Airhouse rather than written
 * into the checkout; the server provisions that tenant itself. Local mode
 * (`oxy serve --local`) keeps every option, with file DuckDB marked deprecated:
 * it is being retired everywhere, and Airhouse replaces it.
 * Plan: `internal-docs/2026-10-29-duckdb-out-of-cloud-workspaces-plan.md`.
 *
 * The signal is `authConfig.mode` from `GET /auth/config` (`"local"` or
 * `"cloud"`), read as `useAuth().isLocalMode`. A dev box (`just up`,
 * `oxy serve --enterprise`) is cloud mode, so it behaves as production does.
 */

/** Database types only local mode may create. */
const LOCAL_ONLY_DATABASE_TYPES: ReadonlySet<string> = new Set(["duckdb"]);

/** The connection types this deployment offers, in the order given. */
export function offeredDatabaseTypes<T extends { type: string }>(
  options: readonly T[],
  isLocalMode: boolean
): T[] {
  if (isLocalMode) return [...options];
  return options.filter((o) => !LOCAL_ONLY_DATABASE_TYPES.has(o.type));
}

/**
 * The secret the onboarding stores a pasted BigQuery key under, for the
 * connection's `key_json_var`. Named after the connection, as the server names
 * a pasted password `<NAME>_PASSWORD`.
 */
export function bigQueryKeySecretName(connectionName: string): string {
  return `${connectionName.toUpperCase()}_KEY_JSON`;
}
