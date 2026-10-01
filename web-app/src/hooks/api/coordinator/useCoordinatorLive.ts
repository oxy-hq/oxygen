import { fetchEventSource } from "@microsoft/fetch-event-source";
import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { previewRequestHeaders } from "@/libs/utils/preview";
import { CoordinatorService } from "@/services/api/coordinator";
import queryKeys from "../queryKey";

/**
 * Subscribes to the coordinator live SSE stream.
 * When a snapshot event arrives, invalidates the active runs query
 * so the UI refreshes automatically.
 */
const useCoordinatorLive = () => {
  const { project } = useCurrentProjectBranch();
  const projectId = project.id;
  const queryClient = useQueryClient();
  const abortRef = useRef<AbortController | null>(null);

  useEffect(() => {
    const controller = new AbortController();
    abortRef.current = controller;
    const token = localStorage.getItem("auth_token");

    fetchEventSource(CoordinatorService.liveStreamUrl(projectId), {
      method: "GET",
      headers: {
        Authorization: token ?? "",
        ...previewRequestHeaders()
      },
      openWhenHidden: true,
      signal: controller.signal,
      async onopen(res) {
        // 401/403 must throw so fetchEventSource stops retrying — otherwise a
        // revoked token hammers the endpoint until the tab is closed.
        if (res.status === 401 || res.status === 403) {
          throw new Error(`Coordinator live auth failed: ${res.status}`);
        }
      },
      onmessage(ev) {
        if (ev.event === "snapshot") {
          // Prefix invalidation covers every active-runs variant
          // (include_system=true and =false) without enumerating them.
          queryClient.invalidateQueries({
            queryKey: [...queryKeys.coordinator.all, "activeRuns", projectId]
          });
        }
      },
      onerror(err) {
        // Intentional aborts (route change, unmount) shouldn't surface as errors.
        // Auth errors thrown from onopen must propagate to stop the retry loop.
        if (controller.signal.aborted || err instanceof Error) {
          throw err;
        }
        // Other transient errors: let fetchEventSource back off and reconnect.
      }
    }).catch(() => {
      // Expected when abort() is called or when onopen/onerror throws.
    });

    return () => {
      controller.abort();
    };
  }, [projectId, queryClient]);
};

export default useCoordinatorLive;
