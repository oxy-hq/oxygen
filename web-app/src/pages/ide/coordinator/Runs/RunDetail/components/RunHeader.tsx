import { ArrowLeft, FileCog } from "lucide-react";
import type React from "react";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import { encodeBase64 } from "@/libs/encoding";
import ROUTES from "@/libs/utils/routes";
import type { TaskTreeNode } from "@/services/api/coordinator";
import useCurrentOrg from "@/stores/useCurrentOrg";
import { isSystemSource, JOB_TYPE, type JobType } from "../../../components/constants";
import { JobTypeBadge } from "../../../components/JobTypeBadge";
import { StatusBadge } from "../../../components/StatusBadge";
import { SystemBadge } from "../../../components/SystemBadge";
import { TriggerBadge } from "../../../components/TriggerBadge";
import { useCoordinatorRoutes } from "../../../components/useCoordinatorRoutes";
import { formatDuration, formatTimestamp } from "../../../components/utils";

const Meta: React.FC<{ label: string; children: React.ReactNode }> = ({ label, children }) => (
  <div className='flex flex-col'>
    <span className='text-muted-foreground text-xs uppercase tracking-wide'>{label}</span>
    <span className='text-sm'>{children}</span>
  </div>
);

/**
 * Shared run-detail header — identical chrome for every job type. The body
 * below it is what goes polymorphic.
 */
export const RunHeader: React.FC<{
  root: TaskTreeNode;
  jobType: JobType;
  nodeCount: number;
}> = ({ root, jobType, nodeCount }) => {
  const routes = useCoordinatorRoutes();
  const isSystem = isSystemSource(root.source_type);
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";

  // Cross-link to the IDE file editor for runs whose source is a
  // YAML file (airway / automation). Hidden when the run has no
  // backing file — analytics agents by `agent_id`, builder runs,
  // preagg daemons.
  const editHref = root.source_ref
    ? ROUTES.ORG(orgSlug).WORKSPACE().IDE.FILES.FILE(encodeBase64(root.source_ref))
    : null;

  return (
    <div className='border-border border-b'>
      <div className='flex items-center gap-3 px-4 py-2.5'>
        <Button asChild variant='ghost' size='icon' className='h-8 w-8'>
          <Link
            to={routes.RUNS}
            aria-label='Back to runs'
            data-testid='coordinator-run-detail-back'
          >
            <ArrowLeft className='h-4 w-4' />
          </Link>
        </Button>
        {isSystem ? <SystemBadge /> : <JobTypeBadge type={jobType} />}
        <h2 className='min-w-0 flex-1 truncate font-semibold text-base'>{root.question}</h2>
        {editHref && (
          <Button asChild variant='outline' size='sm' className='h-8'>
            <Link
              to={editHref}
              data-testid='coordinator-run-detail-edit-yaml'
              title={`Open ${root.source_ref} in the IDE`}
            >
              <FileCog className='h-4 w-4' />
              Edit YAML
            </Link>
          </Button>
        )}
        <StatusBadge status={root.status} />
      </div>
      <div className='flex flex-wrap gap-x-8 gap-y-2 px-4 pb-3'>
        <Meta label='Run ID'>
          <span className='font-mono text-xs'>{root.run_id}</span>
        </Meta>
        <Meta label='Started'>{formatTimestamp(root.created_at)}</Meta>
        <Meta label='Duration'>{formatDuration(root.created_at, root.updated_at)}</Meta>
        <Meta label='Trigger'>
          {root.trigger ? <TriggerBadge trigger={root.trigger} /> : <span>—</span>}
        </Meta>
        <Meta label='Source'>{root.source_type || "—"}</Meta>
        <Meta label='Debugging unit'>{JOB_TYPE[jobType].unit}</Meta>
        <Meta label='Tasks'>{nodeCount}</Meta>
        {root.attempt > 0 && (
          <Meta label='Attempt'>
            <span className='text-warning'>#{root.attempt + 1}</span>
          </Meta>
        )}
      </div>
    </div>
  );
};
