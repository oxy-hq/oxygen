import { useMemo } from "react";
import useAgents from "./useAgents";

/** Fallback when no agent declares a timezone (and the demo/default). The
 *  backend resolves agent→project→UTC, but the product wants Pacific, not
 *  UTC, as the visible default — so the resolution happens here. */
const DEFAULT_WORKSPACE_TIMEZONE = "America/Los_Angeles";

/**
 * The IANA timezone the workspace clock should display, derived from the
 * workspace's default agent's `timezone:` (`.agentic.yml`). Resolves the
 * SAME default agent the ask-locked composer uses (`resolveDefaultAgent`):
 * the first public analytics (`.agentic.yml`) agent, else the first public
 * agent. Falls back to `America/Los_Angeles` when unset or still loading.
 */
export default function useWorkspaceTimezone(): string {
  const { data: agents } = useAgents();
  return useMemo(() => {
    const publicAgents = agents?.filter((a) => a.public) ?? [];
    const resolved =
      publicAgents.find(
        (a) => a.path.endsWith(".agentic.yml") || a.path.endsWith(".agentic.yaml")
      ) ?? publicAgents[0];
    return resolved?.timezone?.trim() || DEFAULT_WORKSPACE_TIMEZONE;
  }, [agents]);
}
