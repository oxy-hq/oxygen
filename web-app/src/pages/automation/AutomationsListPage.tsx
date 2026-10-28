/**
 * Minimal automation file picker — landing page when navigating to
 * `/automations` without a specific path.
 *
 * Lists every `.automation.yml` / `.procedure.yml` known
 * to the workspace and links each to the run page. No quick-launch from
 * here yet — that will come in a follow-up if it earns its weight.
 */

import { Workflow as AutomationIcon, ChevronRight } from "lucide-react";
import { Link } from "react-router-dom";
import PageHeader from "@/components/PageHeader";
import ErrorAlert from "@/components/ui/ErrorAlert";
import LoadingSkeleton from "@/components/ui/LoadingSkeleton";
import { useAgenticAutomationFiles } from "@/hooks/api/agentic-automations/useAgenticAutomations";
import useAppsFirstExperience from "@/hooks/useAppsFirstExperience";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { objectNameFromPath } from "@/libs/utils/objectName";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";

const AutomationsListPage = () => {
  const filesQuery = useAgenticAutomationFiles();
  const { project } = useCurrentProjectBranch();
  // Apps-first: plain names, no file paths or file-creation hints.
  const plainNames = useAppsFirstExperience();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";

  return (
    <div className='flex h-full w-full flex-col' data-testid='automations-list-page'>
      <PageHeader className='items-center border-border border-b-1'>
        <div className='flex w-full items-center gap-2'>
          <AutomationIcon className='size-4' />
          <span className='font-medium text-sm'>Automations</span>
        </div>
      </PageHeader>

      <div className='flex-1 overflow-auto p-4'>
        {filesQuery.isLoading && <LoadingSkeleton className='h-12 w-full' />}
        {filesQuery.isError && (
          <ErrorAlert message={filesQuery.error?.message ?? "Failed to load automations"} />
        )}
        {filesQuery.isSuccess && filesQuery.data.length === 0 && (
          <p className='text-muted-foreground'>
            {plainNames
              ? "No automations yet."
              : "No automations found. Create a `.automation.yml` in the project to get started."}
          </p>
        )}
        {filesQuery.isSuccess && filesQuery.data.length > 0 && (
          <ul className='flex flex-col gap-2'>
            {filesQuery.data.map((file) => (
              <li key={file.path_b64}>
                <Link
                  to={ROUTES.ORG(orgSlug).WORKSPACE(project.id).WORKFLOW(file.path_b64).ROOT}
                  className='flex items-center gap-3 rounded-md border bg-card px-3 py-2 transition-colors hover:bg-muted/50'
                >
                  <AutomationIcon className='size-4 text-muted-foreground' />
                  <span className='flex-1 truncate font-medium text-sm'>
                    {plainNames ? objectNameFromPath(file.path) : file.path}
                  </span>
                  <ChevronRight className='size-4 text-muted-foreground' />
                </Link>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
};

export default AutomationsListPage;
