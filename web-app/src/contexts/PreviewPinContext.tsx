import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useState
} from "react";
import { NavigationType, useLocation, useNavigate, useNavigationType } from "react-router-dom";
import { usePreviews } from "@/hooks/api/workspaces/usePreviews";
import useCanUsePreviews from "@/hooks/useCanUsePreviews";
import { isRevisionToken, PREVIEW_PARAM, setActivePreviewRevision } from "@/libs/utils/preview";

/**
 * The preview pin: which immutable staging REVISION this workspace page is
 * served from.
 *
 * A preview is a MODE of the normal product, not a separate area, so the pin
 * lives in the URL (`?preview=<revision_id>`): a shared link always opens the
 * same compiled revision, and a reload keeps it. It reaches requests two ways:
 *
 *  - `x-oxy-preview-revision: <revision_id>` on every request (published to
 *    the request layer here), which is what makes a request a preview request;
 *  - `useCurrentWorkspaceBranch().branchName` returns the preview's branch
 *    LABEL, which ~160 surfaces send as `?branch=` and fold into their query
 *    keys — so preview data is cached apart from live data.
 *
 * The label comes from the previews list (the row whose `revision_id` is the
 * pin), or straight from whoever entered the preview. Until it is known the
 * page is not rendered at all (`fallback` is): rendering it would fetch with
 * an empty branch — live data, cached as live, under a preview URL.
 *
 * **The pin follows the person, not the link.** Almost every in-app navigation
 * builds a bare path, which would silently drop the query string — and with it
 * the mode — on the first click. So an in-app navigation that loses the param
 * is still pinned (the held pin answers in that same render) and the param is
 * put back. Only two things end a pin: **Exit preview**, and moving through
 * history (back/forward) to an entry that was never pinned.
 *
 * **Oxy staff only** (`useCanUsePreviews`). For anyone else the param is inert.
 */

/** What entering a preview needs: the revision to pin, and what to call it. */
export interface PreviewTarget {
  revisionId: string;
  branch: string;
  sha: string | null;
}

/**
 * `live` — not pinned. `resolving` — pinned, label not known yet (or not yet
 * known whether the viewer may preview). `ready` — pinned and labelled.
 * `unavailable` — pinned to a revision no preview carries.
 */
export type PreviewPinStatus = "live" | "resolving" | "ready" | "unavailable";

export interface PreviewPin {
  status: PreviewPinStatus;
  /** The pinned revision id, or `null` on a live page. */
  revisionId: string | null;
  /** The pinned revision's branch label; `null` unless `status` is `ready`. */
  branch: string | null;
  /** The commit the pinned revision was compiled from, when known. */
  sha: string | null;
  /** Pin `target`, on `pathname` (default: the current page). */
  enter: (target: PreviewTarget, pathname?: string) => void;
  /** Drop the pin and return this same page to live. */
  exit: () => void;
}

const LIVE: PreviewPin = {
  status: "live",
  revisionId: null,
  branch: null,
  sha: null,
  enter: () => {},
  exit: () => {}
};

const PreviewPinContext = createContext<PreviewPin>(LIVE);

/** A usable `?preview=` revision from a search string, or `null`. */
export function readPreviewParam(search: string): string | null {
  const value = new URLSearchParams(search).get(PREVIEW_PARAM)?.trim();
  return value && isRevisionToken(value) ? value : null;
}

/** `search` with the pin set to `revisionId`, or removed when it is null. */
export function withPreviewParam(search: string, revisionId: string | null): string {
  const params = new URLSearchParams(search);
  if (revisionId) params.set(PREVIEW_PARAM, revisionId);
  else params.delete(PREVIEW_PARAM);
  const next = params.toString();
  return next ? `?${next}` : "";
}

interface HeldPin {
  workspaceId: string;
  revisionId: string;
  /** Null until resolved from the list. */
  label: { branch: string; sha: string | null } | null;
}

/** The pinned revision this render, before its label is looked up. */
function usePinnedRevision(workspaceId: string) {
  const location = useLocation();
  const navigate = useNavigate();
  const historyMove = useNavigationType() === NavigationType.Pop;
  const canUsePreviews = useCanUsePreviews();
  const rawPin = readPreviewParam(location.search);
  // Staff only. For anyone else the param is inert: no pin, no header, no
  // `?branch=` — the link opens the live page they would have seen anyway.
  const urlPin = canUsePreviews ? rawPin : null;

  const [held, setHeld] = useState<HeldPin | null>(() =>
    urlPin ? { workspaceId, revisionId: urlPin, label: null } : null
  );
  // The pin being exited, while the URL still carries it for a render or two.
  const [exiting, setExiting] = useState<string | null>(null);

  // A pin belongs to the workspace it was made in: switching workspace by a
  // link that lost the param must not carry a revision of another workspace.
  const heldHere = canUsePreviews && held?.workspaceId === workspaceId ? held : null;

  let revisionId: string | null;
  if (urlPin) revisionId = urlPin === exiting ? null : urlPin;
  else revisionId = historyMove ? null : (heldHere?.revisionId ?? null);

  useEffect(() => {
    if (!urlPin) {
      if (exiting) setExiting(null);
      if (!heldHere) return;
      if (historyMove) {
        setHeld(null);
        return;
      }
      navigate(
        {
          pathname: location.pathname,
          search: withPreviewParam(location.search, heldHere.revisionId),
          hash: location.hash
        },
        { replace: true, state: location.state }
      );
      return;
    }
    if (urlPin === exiting) return;
    if (heldHere?.revisionId !== urlPin) setHeld({ workspaceId, revisionId: urlPin, label: null });
  }, [urlPin, heldHere, exiting, historyMove, workspaceId, location, navigate]);

  return {
    revisionId,
    held: heldHere?.revisionId === revisionId ? heldHere : null,
    setHeld,
    setExiting,
    // Whether the viewer may preview is still unknown, and the link asks for one.
    undecided: canUsePreviews === undefined && rawPin !== null,
    canUsePreviews: !!canUsePreviews,
    location,
    navigate
  };
}

export function PreviewPinProvider({
  workspaceId,
  fallback = null,
  children
}: {
  workspaceId: string;
  /** Rendered instead of `children` while the pin is resolving or unavailable. */
  fallback?: ReactNode;
  children: ReactNode;
}) {
  const pinned = usePinnedRevision(workspaceId);
  const { revisionId, held, setHeld, setExiting, location, navigate } = pinned;

  const list = usePreviews(workspaceId, !!revisionId);
  const row = revisionId ? list.data?.find((p) => p.revision_id === revisionId) : undefined;
  const label = held?.label ?? (row ? { branch: row.branch, sha: row.sha } : null);

  // Keep the label once found: the row moves on (a refresh gives the branch a
  // new revision), but this revision's name does not.
  useEffect(() => {
    if (revisionId && row && !held?.label) {
      setHeld({ workspaceId, revisionId, label: { branch: row.branch, sha: row.sha } });
    }
  }, [revisionId, row, held, setHeld, workspaceId]);

  let status: PreviewPinStatus;
  if (!revisionId) status = pinned.undecided ? "resolving" : "live";
  else if (label) status = "ready";
  else if (list.isSuccess || list.isError) status = "unavailable";
  else status = "resolving";

  // Hand the pin to the request layer (`x-oxy-preview-revision`), which lives
  // outside React. A LAYOUT effect on purpose: React Query subscribes — and so
  // fetches — in passive effects, and every layout effect in a commit runs
  // before any passive one, so the first request a newly pinned (or just
  // exited) page makes already carries the right header. Cleared on unmount,
  // so leaving the workspace never leaves a pin behind.
  useLayoutEffect(() => {
    setActivePreviewRevision(revisionId);
  }, [revisionId]);
  useLayoutEffect(() => () => setActivePreviewRevision(null), []);

  const { canUsePreviews } = pinned;
  const enter = useCallback(
    (target: PreviewTarget, pathname?: string) => {
      if (!canUsePreviews) return;
      const path = pathname ?? location.pathname;
      setExiting(null);
      setHeld({
        workspaceId,
        revisionId: target.revisionId,
        label: { branch: target.branch, sha: target.sha }
      });
      navigate({
        pathname: path,
        search: withPreviewParam(
          path === location.pathname ? location.search : "",
          target.revisionId
        )
      });
    },
    [canUsePreviews, location.pathname, location.search, navigate, setExiting, setHeld, workspaceId]
  );

  const urlRevision = readPreviewParam(location.search);
  const exit = useCallback(() => {
    setExiting(urlRevision);
    setHeld(null);
    navigate(
      {
        pathname: location.pathname,
        search: withPreviewParam(location.search, null),
        hash: location.hash
      },
      { state: location.state }
    );
  }, [urlRevision, location, navigate, setExiting, setHeld]);

  const value = useMemo<PreviewPin>(
    () => ({
      status,
      revisionId,
      branch: status === "ready" ? (label?.branch ?? null) : null,
      sha: status === "ready" ? (label?.sha ?? null) : null,
      enter,
      exit
    }),
    [status, revisionId, label?.branch, label?.sha, enter, exit]
  );

  const showPage = status === "live" || status === "ready";
  return (
    <PreviewPinContext.Provider value={value}>
      {showPage ? children : fallback}
    </PreviewPinContext.Provider>
  );
}

/**
 * The current page's preview pin. Outside a workspace (no provider) this is
 * always live, which keeps every existing caller of `useCurrentWorkspaceBranch`
 * — including the tests that render one without a router — unchanged.
 */
export function usePreviewPin(): PreviewPin {
  return useContext(PreviewPinContext);
}
