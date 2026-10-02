import { fetchEventSource } from "@microsoft/fetch-event-source";
import { useCallback, useEffect, useRef, useState } from "react";
import { previewRequestHeaders } from "@/libs/utils/preview";
import type { UiBlock } from "@/services/api/analytics";
import { AnalyticsService } from "@/services/api/analytics";

interface BuilderDelegationEventsResult {
  events: UiBlock[];
  isStreaming: boolean;
  /** Set when the event stream itself failed (bad status, network) — not a run error. */
  error: Error | null;
}

/**
 * Opens an SSE connection to a child builder run and collects its events.
 * Only connects when `isOpen` is true and `childRunId` is non-null.
 */
export function useBuilderDelegationEvents(
  projectId: string,
  childRunId: string | null,
  isOpen: boolean
): BuilderDelegationEventsResult {
  const [events, setEvents] = useState<UiBlock[]>([]);
  const [isStreaming, setIsStreaming] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const abortRef = useRef<AbortController | null>(null);

  const appendEvent = useCallback((ev: UiBlock) => {
    setEvents((prev) => [...prev, ev]);
  }, []);

  useEffect(() => {
    if (!isOpen || !childRunId) return;

    abortRef.current?.abort();
    const controller = new AbortController();
    abortRef.current = controller;

    const url = AnalyticsService.eventsUrl(projectId, childRunId);
    const token = localStorage.getItem("auth_token");

    setIsStreaming(true);
    setEvents([]);
    setError(null);

    fetchEventSource(url, {
      method: "GET",
      headers: {
        Authorization: token ?? "",
        ...previewRequestHeaders()
      },
      openWhenHidden: true,
      signal: controller.signal,
      async onopen(res) {
        // Throwing routes a refused connection through `onerror`. Returning instead
        // would have the error body read as an empty, finished stream.
        if (res.status !== 200) {
          throw new Error(`Builder delegation event stream failed with status: ${res.status}`);
        }
      },
      onmessage(ev) {
        if (!ev.event) return;
        let parsed: Record<string, unknown> = {};
        try {
          parsed = JSON.parse(ev.data ?? "{}");
        } catch {
          // ignore malformed events
        }
        const block = {
          seq: Number(ev.id) || 0,
          event_type: ev.event,
          payload: parsed
        } as UiBlock;
        appendEvent(block);

        if (ev.event === "done" || ev.event === "error" || ev.event === "cancelled") {
          setIsStreaming(false);
        }
      },
      onerror(err) {
        setIsStreaming(false);
        // Re-throw to stop the library from retrying. The child builder run
        // is either done or unreachable — retrying would exhaust browser
        // connections (HTTP/1.1 limit of ~6) and cancel the parent's SSE.
        throw err;
      },
      onclose() {
        setIsStreaming(false);
      }
    }).catch((streamError: unknown) => {
      // `onerror` rethrows to stop the retries, which rejects this promise. The
      // streaming flag is already cleared there; an abort resolves instead.
      console.error("Builder delegation event stream failed", streamError);
      // A failure of a stream this effect has since replaced is not the current one's.
      if (controller.signal.aborted) return;
      setError(streamError instanceof Error ? streamError : new Error(String(streamError)));
    });

    return () => {
      controller.abort();
    };
  }, [isOpen, childRunId, projectId, appendEvent]);

  return { events, isStreaming, error };
}
