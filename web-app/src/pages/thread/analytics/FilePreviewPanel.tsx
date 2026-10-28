import { Pencil } from "lucide-react";
import type { ReactNode } from "react";
import { useMemo } from "react";
import { Link } from "react-router-dom";
import FileEditor from "@/components/FileEditor";
import { FileEditorProvider } from "@/components/FileEditor/FileEditorContext";
import { useFileEditorContext } from "@/components/FileEditor/useFileEditorContext";
import Markdown from "@/components/Markdown";
import { Panel, PanelContent, PanelHeader } from "@/components/ui/panel";
import { Button } from "@/components/ui/shadcn/button";
import type { BuilderFileChange } from "@/hooks/useBuilderActivity";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { encodeBase64 } from "@/libs/encoding";
import ROUTES from "@/libs/utils/routes";
import { Automation } from "@/pages/automation";
import { EditorContext } from "@/pages/ide/Files/Editor/contexts/EditorContextTypes";
import TopicEditor from "@/pages/ide/Files/Editor/Topic";
import ViewEditor from "@/pages/ide/Files/Editor/View";
import useCurrentOrg from "@/stores/useCurrentOrg";
import { decodeFilePath, detectFileType, FileType } from "@/utils/fileTypes";

const MarkdownFilePreview = () => {
  const {
    state: { content }
  } = useFileEditorContext();
  return (
    <div className='h-full overflow-auto p-6'>
      <Markdown>{content}</Markdown>
    </div>
  );
};

interface FilePreviewPanelProps {
  change: BuilderFileChange;
  onClose: () => void;
}

/** Provides the minimal EditorContext required by View/Topic IDE preview components. */
const MinimalEditorProvider = ({
  pathb64,
  fileType,
  children
}: {
  pathb64: string;
  fileType: FileType;
  children: ReactNode;
}) => {
  const { project, branchName } = useCurrentProjectBranch();
  const value = useMemo(
    () => ({
      pathb64,
      filePath: decodeFilePath(pathb64),
      fileType,
      project,
      branchName,
      isMainEditMode: false,
      gitEnabled: false
    }),
    [pathb64, fileType, project, branchName]
  );
  return <EditorContext.Provider value={value}>{children}</EditorContext.Provider>;
};

const FileTypePreview = ({ filePath }: { filePath: string }) => {
  const pathb64 = encodeBase64(filePath);
  const fileType = detectFileType(filePath);

  switch (fileType) {
    case FileType.PROCEDURE:
    case FileType.AUTOMATION:
      return <Automation pathb64={pathb64} direction='vertical' hideHeader />;
    case FileType.VIEW:
      return (
        <MinimalEditorProvider pathb64={pathb64} fileType={FileType.VIEW}>
          <ViewEditor />
        </MinimalEditorProvider>
      );
    case FileType.TOPIC:
      return (
        <MinimalEditorProvider pathb64={pathb64} fileType={FileType.TOPIC}>
          <TopicEditor />
        </MinimalEditorProvider>
      );
    case FileType.MARKDOWN:
      return (
        <FileEditorProvider pathb64={pathb64}>
          <MarkdownFilePreview />
        </FileEditorProvider>
      );
    default:
      return (
        <FileEditorProvider pathb64={pathb64}>
          <FileEditor readOnly />
        </FileEditorProvider>
      );
  }
};

const FilePreviewPanel = ({ change, onClose }: FilePreviewPanelProps) => {
  const filename = change.filePath.split("/").pop() ?? change.filePath;
  const { project } = useCurrentProjectBranch();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const pathb64 = encodeBase64(change.filePath);
  const ideHref = project
    ? ROUTES.ORG(orgSlug).WORKSPACE(project.id).IDE.FILES.FILE(pathb64)
    : null;

  const actions = ideHref ? (
    <Button
      asChild
      variant='ghost'
      size='icon'
      className='h-7 w-7 shrink-0'
      aria-label='View in IDE'
    >
      <Link to={ideHref}>
        <Pencil className='h-4 w-4' />
      </Link>
    </Button>
  ) : undefined;

  return (
    <Panel>
      <PanelHeader
        title={filename}
        subtitle={change.filePath}
        actions={actions}
        onClose={onClose}
      />
      <PanelContent padding={false}>
        <FileTypePreview filePath={change.filePath} />
      </PanelContent>
    </Panel>
  );
};

export default FilePreviewPanel;
