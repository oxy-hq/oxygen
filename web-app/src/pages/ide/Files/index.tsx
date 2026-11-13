import { Eye } from "lucide-react";
import { useNavigate, useParams } from "react-router-dom";
import { CopyButton } from "@/components/CopyBlock";
import ErrorAlert from "@/components/ui/ErrorAlert";
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup
} from "@/components/ui/shadcn/resizable";
import { Spinner } from "@/components/ui/shadcn/spinner";
import useRevisionFiles from "@/hooks/api/revisionFiles/useRevisionFiles";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { decodeBase64, encodeBase64 } from "@/libs/encoding";
import ROUTES from "@/libs/utils/routes";
import type { RevisionFileEntry } from "@/services/api/revisionFiles";
import useCurrentOrg from "@/stores/useCurrentOrg";
import WorkspaceRepositoryPanel from "../WorkspaceRepositoryPanel";
import { oxycCloneCommand } from "../WorkspaceRepositoryPanel/utils";
import FileTree from "./components/FileTree";
import FileViewer from "./components/FileViewer";
import { apiErrorMessage } from "./errors";

/**
 * The Factory's file browser: the served revision's files, read-only. It reads
 * the compiled workspace (a pinned preview's, when pinned), never a working
 * copy, git or GitHub. Changing a file is clone, edit, push: with nothing
 * selected the page shows the repository and how to open it.
 */
const FilesPage = () => {
  const navigate = useNavigate();
  const { project } = useCurrentProjectBranch();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const files = useRevisionFiles();
  const selectedPath = useSelectedPath();

  const cloneCommand = oxycCloneCommand(project.id);
  const filesRoutes = ROUTES.ORG(orgSlug).WORKSPACE(project.id).IDE.FILES;
  const open = (entry: RevisionFileEntry) => navigate(filesRoutes.FILE(encodeBase64(entry.path)));

  return (
    <div className='flex min-h-0 flex-1 flex-col' data-testid='ide-files-page'>
      <div className='flex shrink-0 flex-wrap items-center gap-x-2 gap-y-1 border-b px-4 py-2 text-sm'>
        <Eye className='size-4 text-muted-foreground' aria-hidden />
        <span className='font-medium'>Files</span>
        <span className='text-muted-foreground'>
          Read-only: what the workspace serves. To change a file, clone it, edit and push:
        </span>
        <span className='flex items-center gap-1' data-testid='ide-files-clone-hint'>
          <code className='rounded bg-muted px-1.5 py-0.5 font-mono text-xs'>{cloneCommand}</code>
          <CopyButton
            text={cloneCommand}
            label='Copy oxyc workspace clone command'
            testId='ide-files-clone-copy'
          />
        </span>
      </div>
      <ResizablePanelGroup direction='horizontal' className='min-h-0 flex-1'>
        <ResizablePanel defaultSize={24} minSize={15} maxSize={45}>
          <div className='flex h-full min-h-0 flex-col'>
            {files.isPending ? (
              <div className='flex justify-center p-4'>
                <Spinner />
              </div>
            ) : files.isError ? (
              <div className='p-3'>
                <ErrorAlert title='Could not list files' message={apiErrorMessage(files.error)} />
              </div>
            ) : files.data.length === 0 ? (
              <p className='p-4 text-muted-foreground text-sm'>The served revision has no files.</p>
            ) : (
              <FileTree entries={files.data} selectedPath={selectedPath} onSelect={open} />
            )}
          </div>
        </ResizablePanel>
        <ResizableHandle withHandle />
        <ResizablePanel defaultSize={76} minSize={40}>
          <div className='flex h-full min-h-0 flex-col'>
            {selectedPath ? <FileViewer path={selectedPath} /> : <WorkspaceRepositoryPanel />}
          </div>
        </ResizablePanel>
      </ResizablePanelGroup>
    </div>
  );
};

/** The file the URL names (`/ide/files/<base64 path>`), or none. */
function useSelectedPath(): string | null {
  const { "*": encoded } = useParams();
  if (!encoded) return null;
  try {
    return decodeBase64(encoded);
  } catch {
    return null;
  }
}

export default FilesPage;
