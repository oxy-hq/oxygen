/**
 * The DuckDB-WASM bundles served from our own origin, for the dev server only.
 *
 * These four `?url` imports are the reason this file exists at all. A `?url`
 * import is an asset *emission*: Vite copies the target into `dist/` whether or
 * not the code that reads the URL ever runs. Held as static imports in
 * `duckdb.ts` they put `duckdb-mvp.wasm` (37.5 MiB) and `duckdb-eh.wasm`
 * (32.7 MiB) plus their two workers into every production build — 71.7 MiB of
 * an 83 MiB `dist/`, embedded by `include_dir!` into every `oxy` binary
 * (`crates/server/src/commands/serve.rs`), and never requested, because the
 * runtime picks the jsDelivr bundles off the dev server.
 *
 * Isolated here and reached only through `await import()` under
 * `import.meta.env.DEV`, the whole module is unreachable in a production build:
 * Vite substitutes `false`, Rollup drops the branch, and nothing pulls the
 * assets in. `dist/assets/*.wasm` is empty in a production build and
 * `web-app/src/libs/duckdb.test.ts` asserts the CDN path is what a
 * non-dev build takes.
 *
 * Keep every `?url` import in this file. One of them in `duckdb.ts` puts the
 * 71.7 MiB back.
 */

import type * as duckdb from "@duckdb/duckdb-wasm";
import eh_worker from "@duckdb/duckdb-wasm/dist/duckdb-browser-eh.worker.js?url";
import mvp_worker from "@duckdb/duckdb-wasm/dist/duckdb-browser-mvp.worker.js?url";
import duckdb_wasm_eh from "@duckdb/duckdb-wasm/dist/duckdb-eh.wasm?url";
import duckdb_wasm from "@duckdb/duckdb-wasm/dist/duckdb-mvp.wasm?url";

export const localBundles: duckdb.DuckDBBundles = {
  mvp: {
    mainModule: duckdb_wasm,
    mainWorker: mvp_worker
  },
  eh: {
    mainModule: duckdb_wasm_eh,
    mainWorker: eh_worker
  }
};
