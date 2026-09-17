// Platform Canary — the page.
//
// Runs the three checks in src/checks.ts once on mount, in order, and renders
// the result as the marker the release checks' browser check waits for.
// Which attributes `<main>` carries is decided in src/marker.ts.

import { useFunction, useQuery } from "@oxy-hq/sdk";
import { type CSSProperties, useEffect, useState } from "react";
import {
  type CheckName,
  checkEcho,
  checkSdkQuery,
  checkSqlQueryRoute,
  DATABASE,
  SELECT_ONE
} from "./checks";
import { CHECK_ORDER, canaryMarker, type Outcome } from "./marker";

type Row = Record<string, unknown>;

interface Failure {
  check: CheckName;
  message: string;
}

export function App() {
  const echo = useFunction("echo");
  const sdkRows = useSettledQuery();
  const [outcomes, setOutcomes] = useState<Partial<Record<CheckName, Outcome>>>({});
  const [failure, setFailure] = useState<Failure | null>(null);

  useEffect(() => {
    let cancelled = false;
    const run: Record<CheckName, () => Promise<void>> = {
      echo: () => checkEcho(echo.invoke),
      sql_query_route: () => checkSqlQueryRoute(),
      sdk_query: () => checkSdkQuery(sdkRows)
    };
    (async () => {
      for (const check of CHECK_ORDER) {
        try {
          await run[check]();
        } catch (err) {
          if (cancelled) return;
          setOutcomes((o) => ({ ...o, [check]: "fail" }));
          setFailure({ check, message: err instanceof Error ? err.message : String(err) });
          return;
        }
        if (cancelled) return;
        setOutcomes((o) => ({ ...o, [check]: "pass" }));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [echo.invoke, sdkRows]);

  return (
    <main {...canaryMarker({ manifest: "loaded", outcomes })} style={styles.main}>
      <h1 style={styles.title}>Platform Canary</h1>
      <ul style={styles.list}>
        {CHECK_ORDER.map((check) => (
          <li key={check}>
            {mark(outcomes[check])} {check}
          </li>
        ))}
      </ul>
      {failure && <pre style={styles.failure}>{failure.message}</pre>}
    </main>
  );
}

/** The SDK's `useQuery` as a promise that settles once the hook stops loading. */
function useSettledQuery(): Promise<Row[]> {
  const query = useQuery<Row>({ sql: SELECT_ONE, database: DATABASE });
  const [deferred] = useState(() => defer<Row[]>());
  useEffect(() => {
    if (query.loading) return;
    if (query.error) deferred.reject(query.error);
    else deferred.resolve(query.rows);
  }, [query.loading, query.error, query.rows, deferred]);
  return deferred.promise;
}

function defer<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  // Handled here so an earlier check's failure, which stops before awaiting this
  // one, does not also log an unhandled rejection.
  promise.catch(() => {});
  return { promise, resolve, reject };
}

function mark(outcome: Outcome | undefined): string {
  if (outcome === "pass") return "[pass]";
  if (outcome === "fail") return "[FAIL]";
  return "[ .. ]";
}

const styles: Record<string, CSSProperties> = {
  main: { fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace", padding: "1.5rem" },
  title: { fontSize: "1.1rem", margin: "0 0 1rem" },
  list: { listStyle: "none", margin: 0, padding: 0, lineHeight: 1.8 },
  failure: { whiteSpace: "pre-wrap", color: "#b42318", marginTop: "1rem" }
};
