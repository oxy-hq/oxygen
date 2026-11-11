// API-backed chat history for the Ask dock. Two bundle-gated reads:
//
//   GET /api/projects/:id/threads          → the viewer's recent threads
//   GET /api/projects/:id/threads/:tid      → a thread's transcript
//
// The transcript returns each turn's events already run through the same
// processor the live SSE stream uses, so the dock rebuilds the reasoning
// trace, charts, and answer with `buildTraceSteps` / the chart filter /
// `extractAnswer` — identical to a live run, and to what the
// thread-history endpoint returns.

import * as React from "react";
import { getOxyAppLogger } from "../custom-app/logger";
import { type AgentRunEvent, type AppFetcher, useOxyApp } from "../custom-app/react";

export interface ThreadSummary {
  id: string;
  title: string;
  created_at: string;
}

export interface TranscriptTurn {
  question: string;
  events: AgentRunEvent[];
}

export interface ThreadTranscript {
  title: string;
  turns: TranscriptTurn[];
}

export interface UseThreadHistoryResult {
  threads: ThreadSummary[];
  loading: boolean;
  error: Error | null;
  /** Re-fetch the list (call after a new chat is archived server-side). */
  refetch: () => void;
}

/**
 * List the viewer's persistent chat threads for the current project.
 * Non-fatal on failure (older server / no session): returns an empty
 * list so the dock falls back to session-only history.
 */
export function useThreadHistory(): UseThreadHistoryResult {
  const { projectId, fetcher } = useOxyApp();
  const [state, setState] = React.useState<{
    threads: ThreadSummary[];
    loading: boolean;
    error: Error | null;
  }>({ threads: [], loading: false, error: null });
  const [nonce, setNonce] = React.useState(0);
  const refetch = React.useCallback(() => setNonce((n) => n + 1), []);

  // biome-ignore lint/correctness/useExhaustiveDependencies: `nonce` is a manual refetch trigger — not read inside the effect, but bumping it must re-run the fetch (projectId/fetcher are stable across a session)
  React.useEffect(() => {
    if (!projectId) {
      setState({ threads: [], loading: false, error: null });
      return;
    }
    let cancelled = false;
    setState((s) => ({ ...s, loading: true }));
    fetcher(`/api/projects/${projectId}/threads`, { method: "GET" })
      .then(async (resp) => {
        if (!resp.ok) throw new Error(`threads list failed: HTTP ${resp.status}`);
        const threads = (await resp.json()) as ThreadSummary[];
        if (!cancelled) setState({ threads, loading: false, error: null });
      })
      .catch((e: unknown) => {
        const error = e instanceof Error ? e : new Error(String(e));
        getOxyAppLogger().log("warn", "thread history unavailable", { error: error.message });
        if (!cancelled) setState({ threads: [], loading: false, error });
      });
    return () => {
      cancelled = true;
    };
  }, [projectId, fetcher, nonce]);

  return { ...state, refetch };
}

/**
 * Fetch a thread's transcript for restore. Throws on failure so the
 * caller can surface a toast / fall back.
 */
export async function fetchThreadTranscript(
  fetcher: AppFetcher,
  projectId: string,
  threadId: string
): Promise<ThreadTranscript> {
  const resp = await fetcher(`/api/projects/${projectId}/threads/${threadId}`, { method: "GET" });
  if (!resp.ok) throw new Error(`thread transcript failed: HTTP ${resp.status}`);
  return (await resp.json()) as ThreadTranscript;
}
