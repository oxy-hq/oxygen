import { ChevronRight, File } from "lucide-react";
import React from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { toast } from "sonner";
import useFileTree from "@/hooks/api/files/useFileTree";
import useRenameFile from "@/hooks/api/files/useRenameFile";
import useRenameFolder from "@/hooks/api/files/useRenameFolder";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { decodeBase64, encodeBase64 } from "@/libs/encoding";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import type { FileTreeModel } from "@/types/file";

interface RenameNodeProps {
  fileTree: FileTreeModel;
  onRenamed: () => void;
  onCancel: () => void;
}

const RenameNode = React.forwardRef<HTMLInputElement, RenameNodeProps>(
  ({ fileTree, onRenamed, onCancel }, ref) => {
    const { pathname } = useLocation();
    const { project } = useCurrentProjectBranch();
    const projectId = project.id;
    const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";

    const { data } = useFileTree();
    const [editingName, setEditingName] = React.useState(fileTree.name);
    const renameFolder = useRenameFolder();
    const renameFile = useRenameFile();
    const navigate = useNavigate();
    const [error, setError] = React.useState(false);

    const onValidateName = (newPath: string) => {
      const pathExistsInTree = (items: FileTreeModel[]): boolean => {
        for (const item of items) {
          if (item.path === newPath) return true;
          if (item.children?.length) {
            if (pathExistsInTree(item.children)) return true;
          }
        }
        return false;
      };

      return !pathExistsInTree(data?.primary || []);
    };

    const isDir = fileTree.is_dir;

    const handleRename = async () => {
      if (editingName !== fileTree.name) {
        try {
          const dirPath = fileTree.path.substring(0, fileTree.path.lastIndexOf("/") + 1);
          const newPath = dirPath + editingName;

          if (!onValidateName(newPath)) {
            setError(true);
            return;
          }

          if (isDir) {
            await renameFolder.mutateAsync({
              pathb64: encodeBase64(fileTree.path),
              newName: newPath
            });
          } else {
            await renameFile.mutateAsync({
              pathb64: encodeBase64(fileTree.path),
              newName: newPath
            });
          }
          const currentPath = decodeBase64(pathname.split("/").pop() ?? "");
          if (currentPath.startsWith(fileTree.path)) {
            const newUrl = currentPath.replace(fileTree.path, newPath);
            const ideUri = ROUTES.ORG(orgSlug)
              .WORKSPACE(projectId)
              .IDE.FILES.FILE(encodeBase64(newUrl));
            navigate(ideUri);
          }
          setError(false);
          onRenamed();
        } catch (error) {
          toast.error("Failed to rename", {
            description: "There was a problem with your request."
          });
          console.error("Failed to rename:", error);
          setEditingName(fileTree.name);
        }
      } else {
        onRenamed();
      }
    };

    const handleKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
      if (e.key === "Enter") {
        // `handleRename` catches a failed rename itself and toasts it.
        void handleRename();
      }
      if (e.key === "Escape") {
        onCancel();
      }
    };

    return (
      <div className='flex w-full flex-col'>
        <div className='flex w-full items-center gap-2 p-2 text-sm'>
          {isDir ? <ChevronRight className='h-4 w-4' /> : <File className='h-4 w-4' />}
          <div className='relative flex-1'>
            <input
              ref={ref}
              onBlur={onCancel}
              className={`w-full border-2 bg-transparent shadow-none outline-none ${
                error ? "border-destructive" : "border-input"
              }`}
              value={editingName}
              onChange={(e) => {
                setEditingName(e.target.value);
                setError(false);
              }}
              onKeyDown={handleKeyDown}
            />
            {error && (
              <div className='absolute top-full left-0 z-10 w-full border border-destructive/20 bg-error p-1 text-foreground text-xs'>
                A {isDir ? "folder" : "file"} with this name already exists
              </div>
            )}
          </div>
        </div>
      </div>
    );
  }
);

export default RenameNode;
