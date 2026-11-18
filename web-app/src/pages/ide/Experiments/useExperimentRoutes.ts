import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";

/** The experiments route table scoped to the current org + workspace. */
export const useExperimentRoutes = () => {
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  return ROUTES.ORG(orgSlug).WORKSPACE().IDE.EXPERIMENTS;
};
