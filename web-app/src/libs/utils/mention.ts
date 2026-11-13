import { IGNORE_FILES_REGEX } from "@/libs/workspaceFiles/constants";
import type { FileTreeModel } from "@/types/file";

export function getCleanObjectName(fileName: string) {
  return fileName
    .replace(/\.(procedure|workflow|automation|agent|aw|app|view|topic)\.(yml|yaml)$/, "")
    .replace(/\.(yml|yaml|sql)$/, "");
}

export function flattenFiles(files: FileTreeModel[]): FileTreeModel[] {
  const result: FileTreeModel[] = [];
  const traverse = (nodes: FileTreeModel[]) => {
    for (const node of nodes) {
      if (IGNORE_FILES_REGEX.some((r) => node.name.match(r))) continue;
      if (!node.is_dir) result.push(node);
      if (node.is_dir && node.children) traverse(node.children);
    }
  };
  traverse(files);
  return result;
}

/** Extract the @query the cursor is currently inside, or null. */
export function getActiveMention(value: string, cursorPos: number) {
  const before = value.slice(0, cursorPos);
  const match = before.match(/(^|[\s])@([^\s]*)$/);
  if (!match) return null;
  const query = match[2];
  const startIndex = before.length - query.length - 1; // position of @
  return { query, startIndex };
}
