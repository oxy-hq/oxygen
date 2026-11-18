import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";

/** The coordinator route table scoped to the current org + workspace. */
export const useCoordinatorRoutes = () => {
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  return ROUTES.ORG(orgSlug).WORKSPACE().IDE.COORDINATOR;
};
