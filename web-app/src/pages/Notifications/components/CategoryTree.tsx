import type { ReactNode } from "react";
import { oxygenFirst } from "@/components/Notifications/utils";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { cn } from "@/libs/shadcn/utils";
import type {
  CategoryNode,
  InboxFilter,
  NotificationCatalog,
  NotificationStream
} from "@/types/notifications";
import { sourceKey, sourcesInStream, sourceTestKey } from "../utils";

/** Indent per depth, on the spacing scale: all, a source, its categories, and so on. */
const INDENT = ["pl-2", "pl-4", "pl-7", "pl-10", "pl-13", "pl-16"];

interface NodeProps {
  label: string;
  unread: number;
  depth: number;
  selected: boolean;
  onSelect: () => void;
  testId: string;
  /** A source or "All". */
  strong?: boolean;
  /** A kind: quieter than the categories around it. */
  leaf?: boolean;
}

function TreeNode({ label, unread, depth, selected, onSelect, testId, strong, leaf }: NodeProps) {
  return (
    <button
      type='button'
      onClick={onSelect}
      aria-current={selected ? "true" : undefined}
      data-testid={testId}
      className={cn(
        "flex w-full items-center gap-2 rounded-md py-1.5 pr-2 text-left text-sm hover:bg-muted/50",
        INDENT[Math.min(depth, INDENT.length - 1)],
        strong && "font-medium",
        selected
          ? "bg-accent text-accent-foreground"
          : leaf
            ? "text-muted-foreground"
            : "text-foreground"
      )}
    >
      <span className='min-w-0 flex-1 truncate'>{label}</span>
      {unread > 0 && (
        <span className='shrink-0 text-muted-foreground text-xs tabular-nums'>{unread}</span>
      )}
    </button>
  );
}

interface CategoryTreeProps {
  catalog: NotificationCatalog | undefined;
  isPending: boolean;
  isError: boolean;
  filter: InboxFilter;
  /** A node was picked: narrows the list to it (and drops any thread). */
  onSelect: (filter: InboxFilter) => void;
  /** The stream whose categories to draw; `undefined` draws every one. */
  stream?: NotificationStream;
}

/**
 * Sources → categories → kinds, each with its unread count. Picking a node
 * narrows the list to its subtree; "All" clears the filter. The desktop
 * sidebar and the phone's filter sheet both draw this.
 *
 * With a `stream`, only that stream's categories are drawn, so the tree beside
 * the Operations tab is Billing and Automations and nothing else. Selecting a
 * node keeps the stream (it is part of the filter the page holds), which is why
 * there is no category here that belongs to the other tab.
 */
export function CategoryTree({
  catalog,
  isPending,
  isError,
  filter,
  onSelect,
  stream
}: CategoryTreeProps) {
  const sources = oxygenFirst(sourcesInStream(catalog?.sources ?? [], stream));
  const total = sources.reduce((sum, s) => sum + s.unread, 0);
  const nothingPicked = !filter.source;

  let body: ReactNode = null;
  if (isPending) {
    body = <Skeleton className='mx-2 h-24' />;
  } else if (isError) {
    body = (
      <p className='px-2 py-2 text-muted-foreground text-xs' data-testid='notifications-tree-error'>
        Couldn't load categories
      </p>
    );
  } else {
    body = sources.map((source) => {
      const key = sourceKey(source);
      const testKey = sourceTestKey(source);
      const here = filter.source === key;
      const categories = (nodes: CategoryNode[], depth: number): ReactNode[] =>
        nodes.flatMap((node) => [
          <TreeNode
            key={`c:${node.path}`}
            label={node.label}
            unread={node.unread}
            depth={depth}
            selected={here && filter.category === node.path && !filter.kind}
            onSelect={() => onSelect({ stream, source: key, category: node.path })}
            testId={`notifications-tree-${testKey}-cat-${node.path}`}
          />,
          ...node.kinds.map((kind) => (
            <TreeNode
              key={`k:${node.path}:${kind.kind}`}
              label={kind.label}
              unread={kind.unread}
              depth={depth + 1}
              leaf
              selected={here && filter.kind === kind.kind}
              onSelect={() => onSelect({ stream, source: key, kind: kind.kind })}
              testId={`notifications-tree-${testKey}-kind-${kind.kind}`}
            />
          )),
          ...categories(node.categories, depth + 1)
        ]);
      return (
        <div key={key} className='flex flex-col'>
          <TreeNode
            label={source.label}
            unread={source.unread}
            depth={1}
            strong
            selected={here && !filter.category && !filter.kind}
            onSelect={() => onSelect({ stream, source: key })}
            testId={`notifications-tree-${testKey}`}
          />
          {categories(source.categories, 2)}
        </div>
      );
    });
  }

  return (
    <nav
      aria-label='Notification categories'
      className='flex flex-col gap-1'
      data-testid='notifications-tree'
    >
      <TreeNode
        label='All'
        unread={total}
        depth={0}
        strong
        selected={nothingPicked}
        onSelect={() => onSelect({ stream })}
        testId='notifications-tree-all'
      />
      {body}
    </nav>
  );
}
