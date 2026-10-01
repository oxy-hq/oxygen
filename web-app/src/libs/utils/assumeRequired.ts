/**
 * Ephemeral, app-wide signal that a request was refused because Oxy staff need
 * an explicit assume-role session for the owning org.
 *
 * Driven from the Axios interceptor (a non-React source) and surfaced to React
 * via `useSyncExternalStore` — same shape as `ideHealth`, so it is neither
 * server state (React Query) nor a Zustand store; just a tiny pub/sub flag the
 * global `ActingShell` subscribes to.
 *
 * Detection contract (stamped by the backend, see
 * `crates/app/src/server/api/middlewares/workspace_context.rs`):
 *   - a `403` carrying `x-oxy-assume-required: <org_id>`
 *   - body: `{ reason: "assume_role_required", org_id, org_name, message }`
 *
 * Why this exists: staff are not org members, so the workspace middleware
 * refuses them by design (the silent override closed in #2710). A bare "you
 * don't have permission" reads as a bug and sends operators hunting through
 * releases; this carries enough context to explain the boundary AND offer the
 * one action that resolves it.
 */

export interface AssumeRequiredState {
  /** The org whose role must be assumed, or `null` when nothing is pending. */
  org: { id: string; name: string } | null;
  /** The server's explanation, shown verbatim so backend and UI never drift. */
  message: string | null;
}

const INITIAL: AssumeRequiredState = { org: null, message: null };

let state: AssumeRequiredState = INITIAL;
const listeners = new Set<() => void>();

function setState(next: AssumeRequiredState): void {
  state = next;
  for (const listener of listeners) listener();
}

/**
 * Raise the assume-role prompt for `orgId`. Repeated refusals for the same org
 * are collapsed — a page firing six parallel queries must not stack six
 * prompts.
 */
export function reportAssumeRequired(
  orgId: string,
  orgName: string | null,
  message: string | null
): void {
  if (state.org?.id === orgId) return;
  // Fall back to the id only when the server couldn't resolve a name; the
  // dialog still works, it just reads less warmly.
  setState({ org: { id: orgId, name: orgName || orgId }, message });
}

/** Clear the prompt — the operator either started a session or dismissed it. */
export function clearAssumeRequired(): void {
  if (state !== INITIAL) setState(INITIAL);
}

export function subscribeAssumeRequired(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function getAssumeRequiredSnapshot(): AssumeRequiredState {
  return state;
}
