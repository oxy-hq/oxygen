import type { editor } from "monaco-editor";
import { useMemo, useState } from "react";
import { BaseMonacoEditor } from "@/components/MonacoEditor";
import { getLanguageFromFileName } from "./constants";
import { useGutterDecorations } from "./hooks/useGutterDecorations";
import useMonacoEditor from "./hooks/useMonacoEditor";
import { useFileEditorContext } from "./useFileEditorContext";

export type FileState = "saved" | "modified" | "saving";

interface Props {
  readOnly?: boolean;
  className?: string;
}

const FileEditor = ({ readOnly = false, className }: Props) => {
  const {
    state: { fileName, content, originalContent, showDiff, isLoading, git },
    actions
  } = useFileEditorContext();

  const [editorInstance, setEditorInstance] = useState<editor.IStandaloneCodeEditor | null>(null);

  useMonacoEditor({
    // The Cmd+S command ignores the result, and save cannot reject: a failed
    // save is caught there and puts the file back to "modified".
    saveFile: (onSuccess) => {
      void actions.save(onSuccess);
    }
  });

  // Show git gutter decorations when: git is enabled, not in diff mode, not read-only.
  // originalContent holds the git-committed version of the file.
  useGutterDecorations(
    editorInstance,
    content,
    originalContent,
    git === true && !showDiff && !readOnly
  );

  const language = useMemo(() => getLanguageFromFileName(fileName), [fileName]);

  if (content === null || content === undefined) {
    return null;
  }

  return (
    <BaseMonacoEditor
      path={`file://${fileName}`}
      value={content}
      onChange={actions.setContent}
      onMount={setEditorInstance}
      language={language}
      className={className}
      isLoading={isLoading}
      diffMode={showDiff}
      original={originalContent ?? undefined}
      height={originalContent ? "calc(100% - 50px)" : "100%"}
      options={{
        minimap: { enabled: true },
        scrollBeyondLastLine: true,
        readOnly: showDiff ? true : readOnly,
        ...(readOnly && !showDiff
          ? {
              readOnlyMessage: {
                value: "This branch is protected — create a branch to make edits"
              }
            }
          : {})
      }}
    />
  );
};

export default FileEditor;
