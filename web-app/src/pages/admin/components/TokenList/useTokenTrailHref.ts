import useCurrentUser from "@/hooks/api/users/useCurrentUser";
import ROUTES from "@/libs/utils/routes";
import type { Token } from "@/types/apiToken";
import { navItemReachable } from "../../AdminLayout/adminNav";

/** The audit log narrowed to one token: what was done with it, and its own lifecycle. */
const auditTrailHref = (token: Token): string => `${ROUTES.ADMIN.AUDIT}?token_id=${token.id}`;

/**
 * Where a token's name leads, for a viewer the audit log admits; `undefined` for one it would
 * turn away. The rail asks the same question of the same map, so the link never leads to a page
 * that bounces.
 */
export const useTokenTrailHref = (): ((token: Token) => string) | undefined => {
  const { data: user } = useCurrentUser();
  const seesAudit = navItemReachable(ROUTES.ADMIN.AUDIT, {
    isOwner: user?.is_owner ?? false,
    capabilities: user?.platform_capabilities ?? []
  });
  return seesAudit ? auditTrailHref : undefined;
};
