import { useQuery } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { apiStatus } from "@/libs/apiError";
import { AppBuildChecksService, type BuildCheckFileRef } from "@/services/api/appBuildChecks";
import { buildChecksKeys } from "./buildChecksKeys";

/** What a picture's frame shows: nothing asked yet, the read in flight, the picture,
 *  no such file (never uploaded, or pruned with its build), or a read that failed. */
export type BuildCheckFileState = "idle" | "loading" | "ready" | "missing" | "error";

/** As long as the server lets the browser keep one (`max-age=3600`). */
const FILE_STALE_MS = 3_600_000;

/**
 * One file of an attempt as an `<img>`-ready object URL. The bytes come through the
 * API client (the session rides along; there is no public URL), so each becomes an
 * object URL that is revoked when the picture leaves the screen.
 *
 * `enabled` is how a picture loads lazily: its frame turns it on once it is near the
 * viewport. A `null` name (the check named no file) reads `missing` without a request,
 * and so does a 404; neither is retried.
 */
export function useBuildCheckFile(
  appId: string,
  build: string,
  name: string | null,
  at: BuildCheckFileRef,
  enabled = true
): { url: string | null; state: BuildCheckFileState } {
  const query = useQuery<Blob>({
    queryKey: buildChecksKeys.buildCheckFile(appId, build, at.environment, at.attempt, name ?? ""),
    queryFn: () => AppBuildChecksService.file(appId, build, name as string, at),
    enabled: enabled && !!appId && !!build && !!name,
    staleTime: FILE_STALE_MS,
    retry: false,
    refetchOnWindowFocus: false
  });

  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    if (!query.data) return;
    const next = URL.createObjectURL(query.data);
    setUrl(next);
    return () => {
      URL.revokeObjectURL(next);
      setUrl(null);
    };
  }, [query.data]);

  if (!name) return { url: null, state: "missing" };
  if (query.isError)
    return { url: null, state: apiStatus(query.error) === 404 ? "missing" : "error" };
  if (url) return { url, state: "ready" };
  return { url: null, state: enabled ? "loading" : "idle" };
}
