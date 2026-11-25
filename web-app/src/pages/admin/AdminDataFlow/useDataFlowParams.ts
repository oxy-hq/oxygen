import { useCallback, useMemo } from "react";
import { useSearchParams } from "react-router-dom";
import type { DataFlowLevel, DataFlowQuery } from "@/types/dataFlow";
import { HIDEABLE_KINDS, type HideableKind } from "./dataFlowView";

/** The server's default, sent only when the caller names another. */
export const DEFAULT_DEPTH = 2;

export type DataFlowParams = {
  org: string | null;
  workspace: string | null;
  focus: string | null;
  depth: number;
  level: DataFlowLevel;
  /** Kinds left off the canvas. Drawn only: the server is asked for the same graph. */
  hide: HideableKind[];
  /** Keep the nodes matching this, and their neighbours. */
  q: string;
  /** Groups unfolded by hand, or every group. */
  open: string[] | "all";
};

const KEYS: Record<keyof DataFlowParams, string> = {
  org: "org",
  workspace: "workspace",
  focus: "focus",
  depth: "depth",
  level: "level",
  hide: "hide",
  q: "q",
  open: "open"
};

const KINDS = new Set<string>(HIDEABLE_KINDS.map((k) => k.kind));
const list = (value: string | null) => (value ? value.split(",").filter(Boolean) : []);

/** `?org=&workspace=&focus=&depth=&level=`, read leniently: an unreadable value is its default. */
export function readParams(search: URLSearchParams): DataFlowParams {
  const depth = Number(search.get(KEYS.depth));
  return {
    org: search.get(KEYS.org) || null,
    workspace: search.get(KEYS.workspace) || null,
    focus: search.get(KEYS.focus) || null,
    depth: Number.isInteger(depth) && depth >= 1 && depth <= 6 ? depth : DEFAULT_DEPTH,
    level: search.get(KEYS.level) === "table" ? "table" : "schema",
    hide: list(search.get(KEYS.hide)).filter((k): k is HideableKind => KINDS.has(k)),
    q: search.get(KEYS.q) ?? "",
    open: search.get(KEYS.open) === "all" ? "all" : list(search.get(KEYS.open))
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

const joinedOrNull = (list: string[]) => (list.length ? list.join(",") : null);

/** Each setting as the URL writes it; `null` (its default) leaves it out. */
const ENCODE: { [K in keyof DataFlowParams]: (value: DataFlowParams[K]) => string | null } = {
  org: (v) => v,
  workspace: (v) => v,
  focus: (v) => v,
  depth: (v) => (v === DEFAULT_DEPTH ? null : String(v)),
  level: (v) => (v === "schema" ? null : v),
  hide: joinedOrNull,
  q: (v) => (v.trim() ? v : null),
  open: (v) => (v === "all" ? "all" : joinedOrNull(v))
};

function encode<K extends keyof DataFlowParams>(key: K, value: DataFlowParams[K] | undefined) {
  return value === undefined || value === null ? null : ENCODE[key](value);
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
  for (const key of Object.keys(patch) as (keyof DataFlowParams)[]) {
    const value = encode(key, patch[key]);
    if (value === null) next.delete(KEYS[key]);
    else next.set(KEYS[key], value);
  }
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
