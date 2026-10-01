import { Eye } from "lucide-react";
import { usePreviewPin } from "@/contexts/PreviewPinContext";
import useCanUsePreviews from "@/hooks/useCanUsePreviews";
import type { Workspace, WorkspacePreview } from "@/types/workspace";
import SectionHeader from "../../../components/SectionHeader";
import NewPreviewForm from "./components/NewPreviewForm";
import PreviewSourcesPanel from "./components/PreviewSourcesPanel";
import PreviewsTable from "./components/PreviewsTable";

interface Props {
  workspace: Workspace;
  /** Called once a preview has been entered, so the dialog can get out of the way. */
  onOpened: () => void;
}

/**
 * Settings → Workspace → Previews: every branch compiled for a look in the real
 * product, with the controls to open, recompile or drop one.
 *
 * **Open** does not go anywhere new — it pins the page behind this dialog to the
 * row's compiled revision (a preview is a mode of the product, not a separate
 * area) and closes the dialog, so the person lands on what they were already
 * looking at, now served from that revision.
 */
export default function Previews({ workspace, onOpened }: Props) {
  const canUsePreviews = useCanUsePreviews();
  const { enter } = usePreviewPin();

  // The nav already hides this tab from anyone who is not staff; this is the
  // same defence-in-depth every gated section carries.
  if (!canUsePreviews) return null;

  const open = (preview: WorkspacePreview) => {
    if (!preview.revision_id) return;
    enter({ revisionId: preview.revision_id, branch: preview.branch, sha: preview.sha });
    onOpened();
  };

  return (
    <div className='flex flex-col gap-5' data-testid='settings-previews'>
      <SectionHeader
        icon={Eye}
        title='Previews'
        description='Open the product on a branch, on real data, without making it live. Read-only.'
      />
      <NewPreviewForm workspaceId={workspace.id} />
      <PreviewSourcesPanel workspaceId={workspace.id} />
      <PreviewsTable workspaceId={workspace.id} onOpen={open} />
    </div>
  );
}
