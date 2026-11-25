import { useCallback, useMemo } from "react";
import { useSearchParams } from "react-router-dom";
import type { DataFlowLevel, DataFlowQuery } from "@/types/dataFlow";

/** The server's default, sent only when the caller names another. */
export const DEFAULT_DEPTH = 2;

export type DataFlowParams = {
  org: string | null;
  workspace: string | null;
  focus: string | null;
  depth: number;
  level: DataFlowLevel;
};

const KEYS: Record<keyof DataFlowParams, string> = {
  org: "org",
  workspace: "workspace",
  focus: "focus",
  depth: "depth",
  level: "level"
};

/** `?org=&workspace=&focus=&depth=&level=`, read leniently: an unreadable value is its default. */
export function readParams(search: URLSearchParams): DataFlowParams {
  const depth = Number(search.get(KEYS.depth));
  return {
    org: search.get(KEYS.org) || null,
    workspace: search.get(KEYS.workspace) || null,
    focus: search.get(KEYS.focus) || null,
    depth: Number.isInteger(depth) && depth >= 1 && depth <= 6 ? depth : DEFAULT_DEPTH,
    level: search.get(KEYS.level) === "table" ? "table" : "schema"
  };
}

/** The request for `params`, or `null` until an org is chosen. Defaults are left out. */
export function toQuery(params: DataFlowParams): DataFlowQuery | null {
  if (!params.org) return null;
  return {
    org_id: params.org,
    ...(params.workspace ? { workspace_id: params.workspace } : {}),
    ...(params.focus ? { focus: params.focus } : {}),
    ...(params.focus && params.depth !== DEFAULT_DEPTH ? { depth: params.depth } : {}),
    ...(params.level === "table" ? { level: "table" as const } : {})
  };
}

/**
 * `current` with `patch` applied: a key in the patch is set, or dropped when it is
 * `null` or its default, so a view's URL carries only what it changed.
 */
export function patchSearch(
  current: URLSearchParams,
  patch: Partial<DataFlowParams>
): URLSearchParams {
  const next = new URLSearchParams(current);
  const put = (key: keyof DataFlowParams, value: string | null) => {
    if (value === null) next.delete(KEYS[key]);
    else next.set(KEYS[key], value);
  };
  for (const key of ["org", "workspace", "focus"] as const) {
    if (key in patch) put(key, patch[key] ?? null);
  }
  if (patch.depth !== undefined) {
    put("depth", patch.depth === DEFAULT_DEPTH ? null : String(patch.depth));
  }
  if (patch.level !== undefined) put("level", patch.level === "schema" ? null : patch.level);
  return next;
}

/** The page's state, which lives in the URL so a view can be linked and reloaded. */
export function useDataFlowParams() {
  const [search, setSearch] = useSearchParams();
  const params = useMemo(() => readParams(search), [search]);
  const update = useCallback(
    (patch: Partial<DataFlowParams>) => setSearch((current) => patchSearch(current, patch)),
    [setSearch]
  );
  return { params, update };
}
