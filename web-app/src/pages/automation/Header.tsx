import { Workflow as Automation, Pencil } from "lucide-react";
import type React from "react";
import { useNavigate } from "react-router-dom";
import PageHeader from "@/components/PageHeader";
import { Button } from "@/components/ui/shadcn/button";
import useAppsFirstExperience from "@/hooks/useAppsFirstExperience";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import useFactoryAccess from "@/hooks/useFactoryAccess";
import { encodeBase64 } from "@/libs/encoding";
import { objectNameFromPath } from "@/libs/utils/objectName";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";

type AutomationPageHeaderProps = {
  path: string;
  runId?: string;
};

const AutomationPageHeader: React.FC<AutomationPageHeaderProps> = ({ path, runId }) => {
  // Apps-first: a plain name and no run path; "Edit" only for a Factory user, who
  // is the one it can take anywhere.
  const appsFirst = useAppsFirstExperience();
  const factoryUser = useFactoryAccess();
  const relativePath = appsFirst ? objectNameFromPath(path) : path;
  const showEdit = !appsFirst || factoryUser;
  const pathb64 = encodeBase64(path);
  const navigate = useNavigate();
  const { project } = useCurrentProjectBranch();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";

  return (
    <PageHeader className='items-center gap-2 border-border border-b-1'>
      <div className='hidden flex-1 md:block' />
      <div className='flex min-w-0 flex-1 items-center justify-center gap-1'>
        <Automation className='h-4 w-4 shrink-0' />
        <span className='min-w-0 truncate text-sm'>
          {relativePath}
          {runId && !appsFirst ? `/runs/${runId}` : ""}
        </span>
      </div>
      <div className='flex shrink-0 items-center gap-2 md:flex-1 md:justify-end'>
        {showEdit && (
          <Button
            size='sm'
            variant='ghost'
            aria-label='Edit'
            onClick={() => {
              const fileUri = ROUTES.ORG(orgSlug).WORKSPACE(project.id).IDE.FILES.FILE(pathb64);
              navigate(fileUri);
            }}
          >
            <Pencil className='h-4 w-4' />
            <span className='hidden sm:inline'>Edit</span>
          </Button>
        )}
      </div>
    </PageHeader>
  );
};

export default AutomationPageHeader;
