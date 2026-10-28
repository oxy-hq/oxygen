import { canUseFactory } from "@/libs/utils/factoryAccess";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

/**
 * Whether the server lets the caller use the Oxygen Factory in the current
 * workspace (`factory_access` on the workspace response; true while the answer
 * is absent).
 *
 * A query to a Factory route belongs behind this — `enabled: factoryAccess` — when
 * it can run on a screen a customer reaches (a component mounted for every page,
 * or a page they open). The route answers `403 factory_access_required` to them
 * regardless; this is what stops the request being sent at all.
 */
export default function useFactoryAccess(): boolean {
  const { workspace } = useCurrentWorkspace();
  return canUseFactory(workspace);
}
