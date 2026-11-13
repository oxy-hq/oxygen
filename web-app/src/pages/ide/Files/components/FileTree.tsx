import { expandAllFeature, hotkeysCoreFeature, syncDataLoaderFeature } from "@headless-tree/core";
import { useTree } from "@headless-tree/react";
import {
  ChevronDown,
  ChevronRight,
  ChevronsDownUp,
  ChevronsUpDown,
  FileCode,
  Folder,
  Search
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import { cn } from "@/libs/shadcn/utils";
import type { RevisionFileEntry } from "@/services/api/revisionFiles";
import { ancestorFolders, buildTree, type FolderNode } from "../utils";

interface FileTreeProps {
  entries: RevisionFileEntry[];
  selectedPath: string | null;
  onSelect: (entry: RevisionFileEntry) => void;
}

/** One node as the tree library sees it. */
interface Node {
  name: string;
  folder: boolean;
  entry?: RevisionFileEntry;
  children: string[];
}

const ROOT = "root";
const folderId = (path: string) => `d:${path}`;
const fileId = (path: string) => `f:${path}`;

/** Index a folder tree by item id, which is what the tree library loads from. */
export function indexTree(root: FolderNode): Map<string, Node> {
  const nodes = new Map<string, Node>();
  const visit = (folder: FolderNode, id: string) => {
    nodes.set(id, {
      name: folder.name,
      folder: true,
      children: folder.children.map((c) =>
        c.type === "folder" ? folderId(c.path) : fileId(c.entry.path)
      )
    });
    for (const child of folder.children) {
      if (child.type === "folder") visit(child, folderId(child.path));
      else
        nodes.set(fileId(child.entry.path), {
          name: child.name,
          folder: false,
          entry: child.entry,
          children: []
        });
    }
  };
  visit(root, ROOT);
  return nodes;
}

/** The entries whose path contains `query`, case-insensitive; all of them when it is blank. */
export function filterEntries(entries: RevisionFileEntry[], query: string): RevisionFileEntry[] {
  const q = query.trim().toLowerCase();
  return q ? entries.filter((e) => e.path.toLowerCase().includes(q)) : entries;
}

/**
 * The served revision's files as a folder tree, read-only: selecting a file
 * opens it. Built on headless-tree for keyboard navigation (arrows, Home/End,
 * Enter, `*` to open siblings) and expand/collapse all; the filter narrows the
 * tree to matching paths and opens every folder in what is left.
 *
 * The open file is the URL's, never the library's: there is no selection
 * feature, so a folder click, Ctrl+A or Shift+Arrow cannot move or multiply
 * the highlight away from the file shown on the right.
 */
export default function FileTree({ entries, selectedPath, onSelect }: FileTreeProps) {
  const [query, setQuery] = useState("");
  const shown = useMemo(() => filterEntries(entries, query), [entries, query]);
  const nodes = useMemo(() => indexTree(buildTree(shown)), [shown]);
  const current = selectedPath ? fileId(selectedPath) : null;

  // Only the first render's folders open by default (the top level and the
  // selection's folders); after that the user owns what is open.
  const [initiallyExpanded] = useState(() => [
    ...(nodes.get(ROOT)?.children ?? []).filter((id) => id.startsWith("d:")),
    ...(selectedPath ? ancestorFolders(selectedPath).map(folderId) : [])
  ]);

  const tree = useTree<Node>({
    rootItemId: ROOT,
    getItemName: (item) => item.getItemData().name,
    isItemFolder: (item) => item.getItemData().folder,
    dataLoader: {
      getItem: (id) => nodes.get(id) ?? { name: "", folder: false, children: [] },
      getChildren: (id) => nodes.get(id)?.children ?? []
    },
    initialState: {
      expandedItems: initiallyExpanded,
      focusedItem: current
    },
    onPrimaryAction: (item) => {
      const { entry } = item.getItemData();
      if (entry) onSelect(entry);
    },
    indent: 12,
    features: [syncDataLoaderFeature, hotkeysCoreFeature, expandAllFeature]
  });

  // New data or a new filter: rebuild, and while filtering open everything left.
  // biome-ignore lint/correctness/useExhaustiveDependencies: `nodes` is read by the tree's data loader, not here; a new `nodes` is exactly when the tree must rebuild
  useEffect(() => {
    tree.rebuildTree();
    if (query.trim()) void tree.expandAll();
  }, [tree, nodes, query]);

  return (
    <div className='flex h-full flex-col'>
      <div className='flex shrink-0 items-center gap-1 border-b p-1.5'>
        <div className='relative min-w-0 flex-1'>
          <Search
            className='absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground'
            aria-hidden
          />
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder='Filter files'
            aria-label='Filter files'
            className='h-7 pl-7 text-xs'
            data-testid='ide-files-filter'
          />
        </div>
        <Button
          variant='ghost'
          size='icon'
          className='size-7'
          tooltip={{ content: "Expand all" }}
          aria-label='Expand all'
          onClick={() => void tree.expandAll()}
        >
          <ChevronsUpDown className='size-3.5' />
        </Button>
        <Button
          variant='ghost'
          size='icon'
          className='size-7'
          tooltip={{ content: "Collapse all" }}
          aria-label='Collapse all'
          onClick={() => tree.collapseAll()}
        >
          <ChevronsDownUp className='size-3.5' />
        </Button>
      </div>
      {shown.length === 0 ? (
        <p className='p-3 text-muted-foreground text-xs'>No file matches “{query.trim()}”.</p>
      ) : (
        <div
          {...tree.getContainerProps("Files")}
          className='min-h-0 flex-1 overflow-auto py-1 text-sm outline-none'
          data-testid='ide-files-tree'
        >
          {tree.getItems().map((item) => {
            const data = item.getItemData();
            const Chevron = item.isExpanded() ? ChevronDown : ChevronRight;
            const open = item.getId() === current;
            return (
              <button
                {...item.getProps()}
                key={item.getId()}
                type='button'
                style={{ paddingLeft: `${item.getItemMeta().level * 12 + 8}px` }}
                data-testid={data.folder ? undefined : "ide-files-tree-file"}
                aria-current={open ? "true" : undefined}
                className={cn(
                  "flex w-full items-center gap-1.5 py-1 pr-2 text-left outline-none hover:bg-muted",
                  open && "bg-muted font-medium",
                  item.isFocused() && "ring-1 ring-ring ring-inset"
                )}
              >
                {data.folder ? (
                  <>
                    <Chevron className='size-3.5 shrink-0 text-muted-foreground' aria-hidden />
                    <Folder className='size-3.5 shrink-0 text-muted-foreground' aria-hidden />
                  </>
                ) : (
                  <FileCode className='ml-5 size-3.5 shrink-0 text-muted-foreground' aria-hidden />
                )}
                <span className='truncate'>{item.getItemName()}</span>
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
