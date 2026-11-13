import { ChevronDown, ChevronRight, FileCode, Folder } from "lucide-react";
import { useState } from "react";
import { cn } from "@/libs/shadcn/utils";
import type { RevisionFileEntry } from "@/services/api/revisionFiles";
import type { FolderNode, TreeNode } from "../utils";

interface FileTreeProps {
  root: FolderNode;
  selectedPath: string | null;
  /** Folders open at first render: the top level and the selection's folders. */
  initiallyOpen: string[];
  onSelect: (entry: RevisionFileEntry) => void;
}

/** The served revision's files as a folder tree. Read-only: selecting opens. */
export default function FileTree({ root, selectedPath, initiallyOpen, onSelect }: FileTreeProps) {
  const [open, setOpen] = useState<Set<string>>(() => new Set(initiallyOpen));
  const toggle = (path: string) =>
    setOpen((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });

  return (
    <ul className='py-1 text-sm' data-testid='ide-files-tree'>
      {root.children.map((node) => (
        <TreeItem
          key={nodeKey(node)}
          node={node}
          depth={0}
          open={open}
          toggle={toggle}
          selectedPath={selectedPath}
          onSelect={onSelect}
        />
      ))}
    </ul>
  );
}

const nodeKey = (node: TreeNode) => (node.type === "folder" ? `d:${node.path}` : node.entry.path);

interface TreeItemProps {
  node: TreeNode;
  depth: number;
  open: Set<string>;
  toggle: (path: string) => void;
  selectedPath: string | null;
  onSelect: (entry: RevisionFileEntry) => void;
}

function TreeItem({ node, depth, open, toggle, selectedPath, onSelect }: TreeItemProps) {
  const indent = { paddingLeft: `${depth * 12 + 8}px` };
  if (node.type === "file") {
    const selected = node.entry.path === selectedPath;
    return (
      <li>
        <button
          type='button'
          style={indent}
          onClick={() => onSelect(node.entry)}
          data-testid='ide-files-tree-file'
          aria-current={selected ? "true" : undefined}
          className={cn(
            "flex w-full items-center gap-1.5 py-1 pr-2 text-left hover:bg-muted",
            selected && "bg-muted font-medium"
          )}
        >
          <FileCode className='size-3.5 shrink-0 text-muted-foreground' aria-hidden />
          <span className='truncate'>{node.name}</span>
        </button>
      </li>
    );
  }
  const isOpen = open.has(node.path);
  const Chevron = isOpen ? ChevronDown : ChevronRight;
  return (
    <li>
      <button
        type='button'
        style={indent}
        onClick={() => toggle(node.path)}
        aria-expanded={isOpen}
        className='flex w-full items-center gap-1.5 py-1 pr-2 text-left hover:bg-muted'
      >
        <Chevron className='size-3.5 shrink-0 text-muted-foreground' aria-hidden />
        <Folder className='size-3.5 shrink-0 text-muted-foreground' aria-hidden />
        <span className='truncate'>{node.name}</span>
      </button>
      {isOpen && (
        <ul>
          {node.children.map((child) => (
            <TreeItem
              key={nodeKey(child)}
              node={child}
              depth={depth + 1}
              open={open}
              toggle={toggle}
              selectedPath={selectedPath}
              onSelect={onSelect}
            />
          ))}
        </ul>
      )}
    </li>
  );
}
