import { type JSX, useContext, useEffect, useMemo, useState } from "react";
import FileEditor from "@/components/FileEditor";
import { FileEditorProvider } from "@/components/FileEditor/FileEditorContext";
import { useNavigationBlock } from "@/components/FileEditor/hooks/useNavigationBlock";
import UnsavedChangesDialog from "@/components/FileEditor/UnsavedChangesDialog";
import { useFileEditorContext } from "@/components/FileEditor/useFileEditorContext";
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup
} from "@/components/ui/shadcn/resizable";
import { useSaveToNewBranch } from "@/hooks/api/files/useSaveToNewBranch";
import { decodeBase64 } from "@/libs/encoding";
import { cn } from "@/libs/shadcn/utils";
import { EditorContext } from "@/pages/ide/Files/Editor/contexts/EditorContextTypes";
import EditorHeader from "../EditorHeader";

const MIN_PANE_SIZE_PERCENT = 10;
const NARROW_VIEWPORT_BREAKPOINT = 800;

interface EditorPageWrapperProps {
  pathb64: string;
  headerActions?: JSX.Element;
  headerPrefixAction?: JSX.Element;
  preview?: JSX.Element;
  className?: string;
  pageContentClassName?: string;
  editorClassName?: string;
  git?: boolean;
  defaultDirection?: "horizontal" | "vertical";
  customEditor?: JSX.Element;
  previewOnly?: boolean;
  onSaved?: (content?: string) => void;
  onChanged?: (content: string) => void;
}

const useViewportDetection = (breakpoint: number = NARROW_VIEWPORT_BREAKPOINT) => {
  const [isNarrowViewport, setIsNarrowViewport] = useState(false);

  useEffect(() => {
    const handleResize = () => {
      try {
        setIsNarrowViewport(window.innerWidth < breakpoint);
      } catch {
        // Ignore errors (e.g., SSR)
      }
    };

    handleResize();
    window.addEventListener("resize", handleResize);
    return () => window.removeEventListener("resize", handleResize);
  }, [breakpoint]);

  return isNarrowViewport;
};

const EditorPageWrapper = ({
  pathb64,
  preview,
  headerActions,
  headerPrefixAction,
  className,
  git = false,
  defaultDirection = "horizontal",
  customEditor,
  previewOnly = false,
  onSaved,
  onChanged
}: EditorPageWrapperProps) => {
  const filePath = decodeBase64(pathb64 ?? "");
  const editorCtx = useContext(EditorContext);
  const isMainEditMode = editorCtx?.isMainEditMode ?? false;

  const { saveToNewBranch } = useSaveToNewBranch();

  const isNarrowViewport = useViewportDetection();
  const hasPreview = !!preview;

  const layoutDirection = useMemo(() => {
    return isNarrowViewport ? "vertical" : defaultDirection;
  }, [defaultDirection, isNarrowViewport]);

  const storageKey = `ide:split:${filePath}`;

  const onSaveOverride = useMemo(() => {
    if (!isMainEditMode) return undefined;
    return (pb64: string, content: string, onSuccess?: () => void) =>
      saveToNewBranch(pb64, content, onSuccess);
  }, [isMainEditMode, saveToNewBranch]);

  const renderEditor = () => (
    <div
      className={cn("flex min-h-0 flex-col overflow-hidden bg-background")}
      style={{ width: "100%", height: "100%" }}
    >
      {customEditor ? customEditor : <FileEditor className={customEditor ? "hidden" : ""} />}
    </div>
  );

  const renderPreview = () =>
    preview ? (
      <div
        className='flex min-h-0 flex-1 flex-col overflow-hidden'
        style={{ width: "100%", height: "100%" }}
      >
        {preview}
      </div>
    ) : null;

  return (
    <FileEditorProvider
      pathb64={pathb64}
      git={git}
      onSaved={onSaved}
      onChanged={onChanged}
      onSaveOverride={onSaveOverride}
    >
      <div className='flex h-full flex-1 flex-col overflow-hidden'>
        <EditorHeader
          prefixAction={headerPrefixAction}
          actions={headerActions}
          filePath={filePath}
        />
        <EditorPageWrapperContent
          className={className}
          hasPreview={hasPreview}
          previewOnly={previewOnly}
          storageKey={storageKey}
          layoutDirection={layoutDirection}
          isMainEditMode={isMainEditMode}
          renderEditor={renderEditor}
          renderPreview={renderPreview}
        />
      </div>
    </FileEditorProvider>
  );
};

interface EditorPageWrapperContentProps {
  className?: string;
  hasPreview: boolean;
  previewOnly?: boolean;
  storageKey: string;
  layoutDirection: "horizontal" | "vertical";
  isMainEditMode: boolean;
  renderEditor: () => JSX.Element;
  renderPreview: () => JSX.Element | null;
}

const EditorPageWrapperContent = ({
  className,
  hasPreview,
  previewOnly = false,
  storageKey,
  layoutDirection,
  isMainEditMode,
  renderEditor,
  renderPreview
}: EditorPageWrapperContentProps) => {
  const {
    state: { fileState },
    actions
  } = useFileEditorContext();

  const { unsavedChangesDialogOpen, setUnsavedChangesDialogOpen, blocker } =
    useNavigationBlock(fileState);

  const handleSaveAndNavigate = () => {
    // `actions.save` never rejects: it catches a failed save and puts the file back to
    // "modified", so the blocked navigation just does not proceed.
    void actions.save(() => blocker.proceed?.());
  };

  const renderContent = () => {
    if (previewOnly) {
      return renderPreview();
    }
    if (hasPreview) {
      return (
        <ResizablePanelGroup
          autoSaveId={storageKey}
          direction={layoutDirection}
          className='flex h-full flex-1 overflow-hidden'
        >
          <ResizablePanel minSize={MIN_PANE_SIZE_PERCENT} defaultSize={50}>
            {renderEditor()}
          </ResizablePanel>

          <ResizableHandle withHandle />

          <ResizablePanel>{renderPreview()}</ResizablePanel>
        </ResizablePanelGroup>
      );
    }
    return renderEditor();
  };

  return (
    <>
      <div className={cn("flex min-h-0 flex-1", className)}>{renderContent()}</div>

      <UnsavedChangesDialog
        open={unsavedChangesDialogOpen}
        onOpenChange={setUnsavedChangesDialogOpen}
        isMainEditMode={isMainEditMode}
        onDiscard={() => {
          setUnsavedChangesDialogOpen(false);
          blocker.proceed?.();
        }}
        onSave={handleSaveAndNavigate}
      />
    </>
  );
};

export default EditorPageWrapper;
