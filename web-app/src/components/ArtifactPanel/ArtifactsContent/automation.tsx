/**
 * Automation artifact panel.
 *
 * The legacy panel rendered a full React-Flow diagram of the automation
 * (driven by `useAutomationConfig` + recursive sub-automation expansion).
 * That hook talked to the retired `/automations/{pathb64}` endpoint, so
 * the panel now degrades to a deep link into the rebuilt run page plus
 * the existing logs view, which still works because logs ride along on
 * the artifact itself.
 */

import { ExternalLink } from "lucide-react";
import { Link } from "react-router-dom";
import OutputLogs from "@/components/automation/output/Logs";
import EmptyState from "@/components/ui/EmptyState";
import { Button } from "@/components/ui/shadcn/button";
import useAppsFirstExperience from "@/hooks/useAppsFirstExperience";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { decodeBase64 } from "@/libs/encoding";
import { objectNameFromPath } from "@/libs/utils/objectName";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import type { AutomationArtifact } from "@/types/artifact";

type Props = {
  artifact: AutomationArtifact;
  onArtifactClick?: (id: string) => void;
};

/** Pure header — receives display + nav data, no fetching. */
export const AutomationArtifactHeader = ({
  displayPath,
  href
}: {
  displayPath: string;
  href: string;
}) => (
  <div className='flex items-center justify-between gap-2 border-border border-b px-4 py-2'>
    <div className='flex min-w-0 items-center gap-2'>
      <span className='truncate font-mono text-muted-foreground text-sm'>{displayPath}</span>
    </div>
    <Button asChild size='sm' variant='outline'>
      <Link to={href}>
        Open <ExternalLink className='size-3.5' />
      </Link>
    </Button>
  </div>
);

const AutomationArtifactPanel = ({ artifact, onArtifactClick }: Props) => {
  const { project } = useCurrentProjectBranch();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const pathB64 = artifact.content.value.ref;
  // Apps-first: the automation's plain name, never its file path.
  const plainNames = useAppsFirstExperience();
  const displayPath = plainNames
    ? objectNameFromPath(decodeBase64(pathB64))
    : decodeBase64(pathB64);
  const href = ROUTES.ORG(orgSlug).WORKSPACE(project.id).WORKFLOW(pathB64).ROOT;
  const logs = artifact.content.value.output ?? [];
  return (
    <div className='flex h-full flex-col'>
      <AutomationArtifactHeader displayPath={displayPath} href={href} />
      <div className='flex h-full flex-col bg-sidebar-background'>
        {logs.length === 0 ? (
          <EmptyState
            className='mt-[150px]'
            title='No logs yet'
            description='Run the automation to see the logs'
          />
        ) : (
          <div className='min-h-0 flex-1'>
            <OutputLogs
              onArtifactClick={onArtifactClick}
              isPending={artifact.is_streaming || false}
              logs={logs}
              onlyShowResult={false}
            />
          </div>
        )}
      </div>
    </div>
  );
};

export default AutomationArtifactPanel;
