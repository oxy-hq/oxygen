import { useCallback } from "react";
import { useSearchParams } from "react-router-dom";

/** Every entity the detail stage can render. `workspaces` is a valid detail
 *  target (reached from inside an org) but is deliberately NOT in the switcher —
 *  workspaces live one level down, inside their org. */
export type TenantType = "orgs" | "users" | "workspaces" | "partners";

export type TenantView = "list" | "map";

/** The top-level switcher: the three relationship-connected entities. */
export const SWITCHER_TYPES: { id: TenantType; label: string; short: string }[] = [
  { id: "orgs", label: "Organizations", short: "Orgs" },
  { id: "partners", label: "Partners", short: "Partners" },
  { id: "users", label: "Users", short: "Users" }
];

/** URL-synced selection (`?type=&id=&view=`) so rows deep-link and back/forward work. */
export function useTenantSelection() {
  const [params, setParams] = useSearchParams();
  const type = (params.get("type") as TenantType) || "orgs";
  const id = params.get("id");
  const view = (params.get("view") as TenantView) || "list";
  const partnerFilter = params.get("partner");

  const setType = useCallback(
    (t: TenantType) =>
      setParams(
        (prev) => {
          const next = new URLSearchParams(prev);
          next.set("type", t);
          next.delete("id");
          next.delete("partner");
          return next;
        },
        { replace: true }
      ),
    [setParams]
  );

  const setId = useCallback(
    (nextId: string | null) =>
      setParams((prev) => {
        const next = new URLSearchParams(prev);
        if (nextId) next.set("id", nextId);
        else next.delete("id");
        return next;
      }),
    [setParams]
  );

  const setView = useCallback(
    (v: TenantView) =>
      setParams(
        (prev) => {
          const next = new URLSearchParams(prev);
          next.set("view", v);
          return next;
        },
        { replace: true }
      ),
    [setParams]
  );

  /** The phone's way back from a dossier, to a list that exists: workspaces have
   *  none of their own, so going back from one lands on the organizations. */
  const backToList = useCallback(() => {
    if (type === "workspaces") setType("orgs");
    else setId(null);
  }, [type, setType, setId]);

  /** Select an entity AND, when it's from the map, flip back to the list so its
   *  dossier is visible. Also used to jump from a partner chip to an org. */
  const focus = useCallback(
    (t: TenantType, nextId: string) =>
      setParams((prev) => {
        const next = new URLSearchParams(prev);
        next.set("type", t);
        next.set("id", nextId);
        next.set("view", "list");
        return next;
      }),
    [setParams]
  );

  /** Filter the org list to one partner's orgs (set by clicking a partner chip). */
  const setPartnerFilter = useCallback(
    (partnerId: string | null) =>
      setParams((prev) => {
        const next = new URLSearchParams(prev);
        next.set("type", "orgs");
        if (partnerId) next.set("partner", partnerId);
        else next.delete("partner");
        next.delete("id");
        return next;
      }),
    [setParams]
  );

  return {
    type,
    id,
    view,
    partnerFilter,
    setType,
    setId,
    backToList,
    setView,
    focus,
    setPartnerFilter
  };
}
