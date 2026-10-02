import { useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { toast } from "sonner";
import useCreateFile from "@/hooks/api/files/useCreateFile";
import useFileTree from "@/hooks/api/files/useFileTree";
import useSaveFile from "@/hooks/api/files/useSaveFile";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { encodeBase64 } from "@/libs/encoding";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import type { FileTreeModel } from "@/types/file";

const DEFAULT_TEST_CONTENT = `target: ""

settings:
  runs: 1
  concurrency: 2

cases: []
`;

export const useCreateTestFile = () => {
  const [dialogOpen, setDialogOpen] = useState(false);
  const [fileName, setFileName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [isCreating, setIsCreating] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  const { data: fileTree, refetch } = useFileTree();
  const { project } = useCurrentProjectBranch();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const createFile = useCreateFile();
  const saveFile = useSaveFile();
  const navigate = useNavigate();

  const openDialog = () => {
    setFileName("");
    setError(null);
    setDialogOpen(true);
    setTimeout(() => inputRef.current?.focus(), 0);
  };

  const validate = (name: string): boolean => {
    if (!name.trim()) {
      setError("File name is required");
      return false;
    }
    if (/[<>:"/\\|?*]/.test(name)) {
      setError("File name contains invalid characters");
      return false;
    }
    const fullPath = `${name}.test.yml`;
    const exists = (items: FileTreeModel[] | undefined): boolean => {
      for (const item of items ?? []) {
        if (item.path === fullPath || item.name === fullPath) return true;
        if (item.children?.length && exists(item.children)) return true;
      }
      return false;
    };
    if (exists(fileTree?.primary)) {
      setError("A file with this name already exists");
      return false;
    }
    setError(null);
    return true;
  };

  const handleCreate = async () => {
    if (!validate(fileName)) return;
    setIsCreating(true);
    try {
      const fullPath = `${fileName}.test.yml`;
      const pathb64 = encodeBase64(fullPath);
      await createFile.mutateAsync(pathb64);
      await saveFile.mutateAsync({ pathb64, data: DEFAULT_TEST_CONTENT });
      await refetch();
      setDialogOpen(false);
      setFileName("");
      navigate(ROUTES.ORG(orgSlug).WORKSPACE(project.id).IDE.FILES.FILE(pathb64));
    } catch (err) {
      toast.error("Failed to create test file", {
        description: err instanceof Error ? err.message : "There was a problem creating the file."
      });
    } finally {
      setIsCreating(false);
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") {
      e.preventDefault();
      // `handleCreate` catches its own failures and toasts them.
      void handleCreate();
    }
  };

  return {
    dialogOpen,
    setDialogOpen,
    fileName,
    setFileName,
    error,
    setError,
    isCreating,
    inputRef,
    openDialog,
    handleCreate,
    handleKeyDown
  };
};
