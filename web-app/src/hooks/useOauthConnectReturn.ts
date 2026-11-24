import { useEffect } from "react";
import { useLocation } from "react-router-dom";
import { toast } from "sonner";
import { useOpenSettings } from "@/components/settings/useSettingsRoute";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

/**
 * Closes the loop on a Settings → Connections OAuth redirect.
 *
 * The consent flow leaves the SPA entirely and comes back to `return_path` with
 * `?oxy_connected=ok`. Without this the person lands with no confirmation that anything
 * happened and a stale query string — indistinguishable from having cancelled.
 *
 * The flag goes with the navigation itself — the clean settings address carries no query —
 * so a refresh or a shared link does not re-toast.
 */
export default function useOauthConnectReturn(): void {
  const open = useOpenSettings();
  // The router's location, not `window.location`. On the full page load this runs after they
  // say the same thing, but reading one while navigating through the other is precisely the
  // split that made the old `replaceState` + `open()` pair push a duplicate entry.
  const { search } = useLocation();
  // An OAuth return is a FULL PAGE LOAD, and neither the workspace nor the org
  // store is persisted — so on the first commit after this mounts the role is
  // undefined, and `SettingsDialog` has filtered `workspace.connections` out of
  // the nav (its gate reads `is.workspaceAdmin`, false while undefined).
  //
  // Opening then does not merely fail. `SettingsDialog` falls back to
  // `allItems[0]` and writes that back into the store; with no workspace and no
  // org loaded yet the only surviving group is Preferences, so the selection
  // becomes "preferences.appearance" and STICKS — once details land, that value
  // is legitimately in the nav and the sync effect has nothing to correct.
  //
  // So wait for the role. The effect re-runs when this flips, which is why the
  // early return must sit ABOVE the strip below: the `oxy_connected` flag has to
  // survive the wait, or the reopen has nothing left to act on.
  const roleLoaded = useCurrentWorkspace((s) => s.workspace?.current_user_role) !== undefined;

  useEffect(() => {
    if (!roleLoaded) return;

    if (new URLSearchParams(search).get("oxy_connected") !== "ok") return;

    toast.success("Connected. The refresh token is stored in workspace secrets.");
    // Deliberately NOT also gated on `is.workspaceAdmin`: a non-admin holding
    // this flag would then never have it stripped, leaving the query string
    // dirty indefinitely. `Connections` shows its own access notice for a role
    // known not to be admin, and `authorize` sits behind the WorkspaceAdmin
    // extractor regardless — so opening it is coherent, not a leak.
    // One router-managed step: this both lands on Connections and drops the flag, because
    // the clean address has no query of its own. (QuickBooks also appends `realm_id`, which
    // goes the same way.)
    //
    // It used to be `history.replaceState` to strip the flag, then a plain `open()`. That
    // worked while Connections was a dialog over another page, but settings is an address
    // now: `replaceState` bypasses the router, so the router still believed the location
    // carried `?oxy_connected=ok`, and the `open()` that followed was not a same-page
    // navigation — it *pushed*. History then held the same Connections address twice and
    // the first Back appeared to do nothing. `replaceState({})` also wiped React Router's
    // own `idx`/`key` off the entry the person was standing on.
    open("workspace.connections", null, { replace: true });
  }, [open, roleLoaded, search]);
}
