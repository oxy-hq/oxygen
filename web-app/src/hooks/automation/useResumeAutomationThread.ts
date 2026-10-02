/**
 * Recover a chat-thread automation's logs after a page reload.
 *
 * The active-run path keeps its `LogItem[]` in `useAutomationThreadStore`
 * (zustand). That store is in-memory, so a refresh wipes it. Without
 * recovery the thread/automation page renders an empty `OutputLogs` even
 * though the run actually completed and its events are on disk.
 *
 * This hook fires once per `threadId` mount: if the local zustand entry
 * is empty, it asks the backend for the latest automation run linked to
 * that thread (`agentic_runs.thread_id`), opens an SSE connection to
 * replay every persisted event for that run, and rebuilds the LogItem
 * tree via the same `buildLogItems` aggregator the live runner uses.
 */

import { useEffect } from "react";

import { type AutomationEvent, AutomationService } from "@/services/api/automations";
import useAutomationThreadStore from "@/stores/useAutomationThread";
import { buildLogItems } from "../api/agentic-automations/useLogItems";
import useCurrentProjectBranch from "../useCurrentProjectBranch";

export const useResumeAutomationThread = (threadId: string | undefined) => {
  const { project } = useCurrentProjectBranch();
  const projectId = project.id;
  const { setLogs, setIsLoading, getAutomationThread } = useAutomationThreadStore();

  useEffect(() => {
    if (!threadId) return;
    // Don't disturb an active run: if the store already has logs for this
    // thread (because the user just kicked off the automation from chat
    // and the live SSE is still streaming), the resume path would race
    // with it. The "is there anything to resume" check is local-state-
    // first, so a fresh run that hasn't emitted yet still gets resumed.
    const existing = getAutomationThread(threadId);
    if (existing.logs.length > 0 || existing.isLoading) return;

    const abort = new AbortController();
    let cancelled = false;

    // Cannot reject: the lookup has its own `.catch` and the replay sits in a
    // try/catch; everything between is synchronous store writes.
    void (async () => {
      const latest = await AutomationService.latestRunForThread(projectId, threadId).catch((e) => {
        console.error("resumeAutomationThread: latest-run lookup failed", e);
        return null;
      });
      if (cancelled || !latest) return;

      // The run row drives the loading indicator: if the backend still
      // shows it as in-flight (`running`/`delegating`/etc.), keep the
      // spinner up while we replay; once the SSE stream closes the
      // shared `is_terminal` path finalizes it.
      const isTerminal = ["done", "failed", "cancelled", "timed_out"].includes(
        latest.task_status ?? ""
      );
      setIsLoading(threadId, !isTerminal);

      const events: AutomationEvent[] = [];
      const flush = () => {
        const tree = buildLogItems(events);
        setLogs(threadId, () => tree);
      };

      try {
        await AutomationService.streamEvents(projectId, latest.run_id, {
          signal: abort.signal,
          onEvent: (event) => {
            events.push(event);
            flush();
          }
        });
      } catch (e) {
        if (!abort.signal.aborted) {
          console.error("resumeAutomationThread: SSE replay failed", e);
        }
      } finally {
        if (!cancelled) setIsLoading(threadId, false);
      }
    })();

    return () => {
      cancelled = true;
      abort.abort();
    };
    // setLogs/setIsLoading/getAutomationThread are stable zustand actions.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [threadId, projectId, setLogs, setIsLoading, getAutomationThread]);
};
