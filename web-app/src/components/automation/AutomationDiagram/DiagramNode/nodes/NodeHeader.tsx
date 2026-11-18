import {
  BarChart3,
  Bot,
  CircleAlert,
  CircleHelp,
  Code,
  Database,
  FileText,
  GitBranch,
  Globe,
  LocateFixed,
  Maximize,
  Minimize,
  RefreshCcw,
  Split
} from "lucide-react";
import type { ReactElement } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import TruncatedText from "@/components/TruncatedText";
import { Button } from "@/components/ui/shadcn/button";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { encodeBase64 } from "@/libs/encoding";
import ROUTES from "@/libs/utils/routes";
import {
  type AutomationTaskConfig,
  type NodeType,
  type TaskConfigWithId,
  TaskType
} from "@/stores/useAutomation";
import useCurrentOrg from "@/stores/useCurrentOrg";
import { headerHeight } from "../../layout/constants";
import { OmniIcon } from "./OmniIcon";

const nodeNameMap: Record<NodeType, string> = {
  agent: "Agent",
  execute_sql: "SQL",
  semantic_query: "Semantic Query",
  looker_query: "Looker Query",
  omni_query: "Omni Query",
  loop_sequential: "Loop sequential",
  formatter: "Formatter",
  workflow: "Sub-automation",
  conditional: "Conditional",
  "conditional-else": "Else",
  "conditional-if": "If",
  airway: "Airway"
};

const nodeIconMap: Record<NodeType, ReactElement> = {
  agent: <Bot size={14} />,
  execute_sql: <Code size={14} />,
  loop_sequential: <RefreshCcw size={14} />,
  formatter: <FileText size={14} />,
  workflow: <GitBranch size={14} />,
  conditional: <Split size={14} />,
  "conditional-else": <CircleAlert size={14} />,
  "conditional-if": <CircleHelp size={14} />,
  semantic_query: <Globe size={14} />,
  looker_query: <BarChart3 size={14} />,
  omni_query: <OmniIcon className='h-3.5 w-3.5' />,
  airway: <Database size={14} />
};

type Props = {
  name: string;
  type: NodeType;
  task?: TaskConfigWithId;
  expandable?: boolean;
  expanded?: boolean;
  onExpandClick?: () => void;
};

export const NodeHeader = ({ type, name, task, expandable, expanded, onExpandClick }: Props) => {
  const taskName = nodeNameMap[type];
  const taskIcon = nodeIconMap[type];
  return (
    <div
      className='flex w-full min-w-0 items-center gap-2'
      style={{
        height: headerHeight
      }}
    >
      <div className='flex min-w-0 items-center'>
        <div className='flex items-center justify-center rounded-md bg-oxy-blue-500/20 p-2 text-oxy-blue-500 dark:text-oxy-blue-200'>
          {taskIcon}
        </div>
      </div>
      <div className='flex min-w-0 flex-1 items-center'>
        <div className='flex min-w-0 flex-1 flex-col gap-1'>
          <div className='flex items-center'>
            <span className='truncate text-muted-foreground text-sm'>{taskName}</span>
          </div>
          <div className='flex min-w-0 items-center'>
            <TruncatedText className='min-w-0 text-sm'>{name}</TruncatedText>
          </div>
        </div>
        <div className='flex h-full items-center justify-start'>
          {expandable && (
            <Button className='p-1 ps-1 pe-1' variant='ghost' onClick={onExpandClick}>
              {expanded ? <Minimize size={14} /> : <Maximize size={14} />}
            </Button>
          )}
          {type === TaskType.WORKFLOW && (
            <SubAutomationNavigateButton task={task as AutomationTaskConfig} />
          )}
        </div>
      </div>
    </div>
  );
};

type SubAutomationNavigateButtonProps = {
  task: AutomationTaskConfig;
};

const SubAutomationNavigateButton = ({ task }: SubAutomationNavigateButtonProps) => {
  const location = useLocation();
  const navigate = useNavigate();
  const { project } = useCurrentProjectBranch();
  const projectId = project.id;
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";

  // The legacy per-task `subAutomationRunId` lookup is gone; child
  // sub-automations start fresh on click. Run selection happens on the
  // destination page itself via the run-history dropdown.
  const handleClick = () => {
    if (!projectId) return;
    const pathb64 = encodeBase64(task.src);
    let automationPath = ROUTES.ORG(orgSlug).WORKSPACE().WORKFLOW(pathb64).ROOT;
    const ideRoute = ROUTES.ORG(orgSlug).WORKSPACE().IDE.ROOT;
    if (location.pathname.startsWith(ideRoute)) {
      automationPath = ROUTES.ORG(orgSlug).WORKSPACE().IDE.FILES.FILE(pathb64);
    }
    navigate(automationPath);
  };

  return (
    <Button
      className='p-1 ps-1 pe-1'
      variant='ghost'
      title='Navigate to definition'
      onClick={handleClick}
    >
      <LocateFixed size={14} />
    </Button>
  );
};
