import type { RevisionFileEntry } from "@/services/api/revisionFiles";

export interface FileNode {
  type: "file";
  name: string;
  entry: RevisionFileEntry;
}

export interface FolderNode {
  type: "folder";
  name: string;
  /** Workspace-relative folder path, `""` for the root. */
  path: string;
  children: TreeNode[];
}

export type TreeNode = FileNode | FolderNode;

/** Folders first, then files, each by name. */
const byFolderThenName = (a: TreeNode, b: TreeNode) =>
  a.type === b.type ? a.name.localeCompare(b.name) : a.type === "folder" ? -1 : 1;

/** The flat, path-sorted listing as a folder tree. */
export function buildTree(entries: RevisionFileEntry[]): FolderNode {
  const root: FolderNode = { type: "folder", name: "", path: "", children: [] };
  for (const entry of entries) {
    const parts = entry.path.split("/").filter(Boolean);
    let folder = root;
    for (const part of parts.slice(0, -1)) {
      const path = folder.path ? `${folder.path}/${part}` : part;
      let next = folder.children.find(
        (c): c is FolderNode => c.type === "folder" && c.name === part
      );
      if (!next) {
        next = { type: "folder", name: part, path, children: [] };
        folder.children.push(next);
      }
      folder = next;
    }
    folder.children.push({ type: "file", name: parts[parts.length - 1] ?? entry.path, entry });
  }
  const sort = (folder: FolderNode) => {
    folder.children.sort(byFolderThenName);
    for (const child of folder.children) if (child.type === "folder") sort(child);
  };
  sort(root);
  return root;
}

/** The folder paths that contain `path`, so a selected file's folders open. */
export function ancestorFolders(path: string): string[] {
  const parts = path.split("/").filter(Boolean).slice(0, -1);
  return parts.map((_, i) => parts.slice(0, i + 1).join("/"));
}
