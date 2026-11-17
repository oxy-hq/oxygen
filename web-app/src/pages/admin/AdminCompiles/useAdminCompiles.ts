import { useMemo, useState } from "react";
import { useSearchParams } from "react-router-dom";

import { useCompiles, useCompileWorkspaces } from "@/hooks/api/compiles";

import type { CompileView } from "./components/CompileFilters";

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/**
 * Owns the page's query + filter state and selects the right poller for
 * the active view. The flat view filters by workspace UUID (`workspace_id`
 * param); the rollup view free-text searches name/path/id (`q` param).
 * Both keep the 5s LiveIndicator cadence and honor the shared pause flag.
 */
export function useAdminCompiles() {
  const [view, setView] = useState<CompileView>("workspace");
  const [paused, setPaused] = useState(false);
  // Seeded from `?q=`, so Internal jobs' "Compile history" link opens this page already
  // narrowed to the failing workspace.
  const [params] = useSearchParams();
  const [query, setQuery] = useState(() => params.get("q") ?? "");
  const [status, setStatus] = useState("");

  const trimmed = query.trim();

  const workspaces = useCompileWorkspaces(
    { limit: 50, q: trimmed || undefined, status: status || undefined },
    { paused: paused || view !== "workspace" }
  );

  // The flat list's only server filter is an exact workspace UUID. Only send it
  // when the search box actually holds a UUID — otherwise free text would
  // deserialize-fail (`workspace_id: Option<Uuid>`) into a 400 and blank the
  // table. Non-UUID text simply leaves the list unfiltered.
  const revisions = useCompiles(
    {
      limit: 50,
      workspace_id: UUID_RE.test(trimmed) ? trimmed : undefined,
      status: status || undefined
    },
    { paused: paused || view !== "revisions" }
  );

  const active = view === "workspace" ? workspaces : revisions;

  const workspaceRows = workspaces.data?.rows ?? [];
  const revisionRows = revisions.data?.rows ?? [];

  const totalLabel = useMemo(() => {
    if (view === "workspace") {
      const n = workspaces.data?.total_returned ?? workspaceRows.length;
      return `${n} workspace${n === 1 ? "" : "s"}`;
    }
    const n = revisions.data?.total_returned ?? revisionRows.length;
    return `${n} recent`;
  }, [view, workspaces.data, revisions.data, workspaceRows.length, revisionRows.length]);

  return {
    view,
    setView,
    paused,
    setPaused,
    query,
    setQuery,
    status,
    setStatus,
    workspaces,
    revisions,
    workspaceRows,
    revisionRows,
    active,
    totalLabel
  };
}
