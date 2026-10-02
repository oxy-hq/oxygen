import { isAxiosError } from "axios";
import type React from "react";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import useFile from "@/hooks/api/files/useFile";
import useFileGit from "@/hooks/api/files/useFileGit";
import useSaveFile from "@/hooks/api/files/useSaveFile";
import { apiErrorMessage } from "@/libs/apiError";
import { readDetachedHeadBody } from "@/libs/utils/detachedHead";
import { readPreviewReadOnlyBody } from "@/libs/utils/preview";
import { decodeFilePath } from "@/utils/fileTypes";
import { FileEditorContext } from "./useFileEditorContext";

// The HTTP client already toasts these refusals in the server's own words
// (services/api/axios.ts): a 403, a write refused in a workspace preview, a git
// action on a detached HEAD. A second "Failed to save" would only repeat it.
const reportedByHttpClient = (error: unknown): boolean => {
  if (!isAxiosError(error)) return false;
  const status = error.response?.status;
  const body: unknown = error.response?.data;
  return (
    status === 403 ||
    readPreviewReadOnlyBody(status, body) !== null ||
    readDetachedHeadBody(status, body) !== null
  );
};

interface EditorProviderProps {
  children: React.ReactNode;
  pathb64: string;
  git?: boolean;
  onSaved?: (content?: string) => void;
  onChanged?: (content: string) => void;
  /** When provided, replaces the default save-to-current-branch behaviour.
   *  Receives (pathb64, content, onSuccess) and must resolve when the save is complete. */
  onSaveOverride?: (pathb64: string, content: string, onSuccess?: () => void) => Promise<void>;
}

export function FileEditorProvider({
  children,
  pathb64,
  git = false,
  onSaved,
  onChanged,
  onSaveOverride
}: EditorProviderProps) {
  const { mutate: saveFile } = useSaveFile();
  const fileName = decodeFilePath(pathb64);
  const { data: fileContent, isPending, isSuccess } = useFile(pathb64);
  const [fileState, setFileState] = useState<"saved" | "modified" | "saving">("saved");
  const { data: originalContent } = useFileGit(pathb64, git);
  const [showDiff, setShowDiff] = useState(false);
  const [content, setContent] = useState(fileContent || "");

  useEffect(() => {
    onChanged?.(content);
  }, [content, onChanged]);

  useEffect(() => {
    if (isSuccess && fileContent) {
      setContent(fileContent || "");
    }
  }, [fileContent, isSuccess]);

  // A failed save has to say so. From the "save and navigate" dialog, which has already
  // closed by then, a silent return to "modified" reads as the button doing nothing.
  const reportSaveFailed = (error: unknown) => {
    setFileState("modified");
    console.error("Failed to save file:", error);
    if (reportedByHttpClient(error)) return;
    toast.error(`Failed to save ${fileName.split("/").pop() || "file"}`, {
      description: apiErrorMessage(error, "") || undefined
    });
  };

  const actions = {
    setContent: (newContent: string) => {
      setContent(newContent);
      setFileState("modified");
    },
    setShowDiff: (show: boolean) => {
      setShowDiff(show);
    },
    markSaved: () => {
      setFileState("saved");
    },
    save: async (onSuccess?: () => void) => {
      if (fileState === "saving") return;
      if (onSaveOverride) {
        setFileState("saving");
        try {
          await onSaveOverride(pathb64, content, () => {
            setFileState("saved");
            onSaved?.(content);
            onSuccess?.();
          });
        } catch (error) {
          reportSaveFailed(error);
        }
        return;
      }
      saveFile(
        { pathb64, data: content },
        {
          onSuccess: () => {
            setFileState("saved");
            onSaved?.(content);
            onSuccess?.();
          },
          onError: reportSaveFailed
        }
      );
    }
  };

  const contextValue = {
    state: {
      fileName,
      isLoading: isPending,
      content: content || "",
      originalContent,
      fileState,
      showDiff,
      git
    },
    actions: actions
  };

  return <FileEditorContext.Provider value={contextValue}>{children}</FileEditorContext.Provider>;
}
