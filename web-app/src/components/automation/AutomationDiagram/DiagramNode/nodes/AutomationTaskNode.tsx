import { useNavigate } from "react-router-dom";

import { encodeBase64 } from "@/libs/encoding";
import ROUTES from "@/libs/utils/routes";
import type { AutomationTaskConfigWithId, TaskConfigWithId } from "@/stores/useAutomation";
import useCurrentOrg from "@/stores/useCurrentOrg";
import { NodeHeader } from "./NodeHeader";

type Props = {
  task: TaskConfigWithId;
  /** Unused — sub-automations are now navigated to as their own page; the
   *  legacy in-place expand was removed. Kept on the prop so the
   *  NodeContent dispatch table doesn't need to change. */
  expanded?: boolean;
};

/**
 * Sub-automation task node — clicking the expand chevron navigates to
 * the child automation's run page rather than expanding the diagram
 * in-place. Run selection happens on the destination page via its
 * own run-history dropdown.
 */
export function AutomationTaskNode({ task }: Props) {
  const navigate = useNavigate();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const subSrc = (task as AutomationTaskConfigWithId).src;
  const expandable = !!subSrc;

  const onExpandClick = () => {
    if (!subSrc) return;
    const pathB64 = encodeBase64(subSrc);
    navigate(ROUTES.ORG(orgSlug).WORKSPACE().WORKFLOW(pathB64).ROOT);
  };

  return (
    <NodeHeader
      name={task.name}
      type={task.type}
      task={task}
      expandable={expandable}
      expanded={false}
      onExpandClick={onExpandClick}
    />
  );
}
