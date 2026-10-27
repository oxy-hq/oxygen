import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";

/** The experiments route table scoped to the current org + workspace. */
export const useExperimentRoutes = () => {
  const { project } = useCurrentProjectBranch();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  return ROUTES.ORG(orgSlug).WORKSPACE(project.id).IDE.EXPERIMENTS;
};
