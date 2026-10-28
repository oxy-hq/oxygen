import type { ReactNode } from "react";
import { Navigate } from "react-router-dom";
import { canUseFactory } from "@/libs/utils/factoryAccess";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

/**
 * Sends someone the server will not let into the Oxygen Factory back to the
 * workspace home instead of rendering an IDE whose every request is refused.
 *
 * Presentation only, like the rail entry: the Factory's routes answer 403
 * `factory_access_required` whatever this renders. Renders its children until the
 * workspace says otherwise, so a response that predates `factory_access` changes
 * nothing.
 */
export function FactoryGuard({ children }: { children: ReactNode }) {
  const { workspace } = useCurrentWorkspace();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  if (workspace && !canUseFactory(workspace)) {
    return <Navigate to={ROUTES.ORG(orgSlug).WORKSPACE(workspace.id).HOME} replace />;
  }
  return <>{children}</>;
}
