import { format, isSameDay, isSameYear, startOfDay, subDays } from "date-fns";
import type {
  CategoryNode,
  InboxFilter,
  InboxItem,
  KindNode,
  NotificationCatalog,
  NotificationStream,
  SourceNode
} from "@/types/notifications";

/** A row of the list: the newest notification of its thread, and how many older ones it hides. */
export interface FeedEntry {
  item: InboxItem;
  more: number;
}

/** A thread belongs to one source: two apps may well pick the same thread text. */
function threadKey(item: InboxItem): string | null {
  return item.thread ? `${item.app?.id ?? "oxygen"}:${item.thread}` : null;
}

/**
 * Collapse every thread into its newest notification. `items` are newest
 * first, so the first of a thread is the one that stays; the rest are counted.
 */
export function collapseThreads(items: InboxItem[]): FeedEntry[] {
  const heads = new Map<string, FeedEntry>();
  const entries: FeedEntry[] = [];
  for (const item of items) {
    const key = threadKey(item);
    const head = key ? heads.get(key) : undefined;
    if (head) {
      head.more += 1;
      continue;
    }
    const entry = { item, more: 0 };
    if (key) heads.set(key, entry);
    entries.push(entry);
  }
  return entries;
}

/** "Today", "Yesterday", "Mon, Oct 12" — with the year once it is not this one. */
export function dayLabel(day: Date, now: Date): string {
  if (isSameDay(day, now)) return "Today";
  if (isSameDay(day, subDays(now, 1))) return "Yesterday";
  return format(day, isSameYear(day, now) ? "EEE, MMM d" : "MMM d, yyyy");
}

export interface DayGroup {
  key: string;
  label: string;
  entries: FeedEntry[];
}

/** Consecutive entries by the local day they arrived, in the order given. */
export function groupByDay(entries: FeedEntry[], now: Date): DayGroup[] {
  const groups: DayGroup[] = [];
  for (const entry of entries) {
    const date = new Date(entry.item.created_at);
    const valid = !Number.isNaN(date.getTime());
    const key = valid ? format(startOfDay(date), "yyyy-MM-dd") : "unknown";
    const last = groups.at(-1);
    if (last?.key === key) {
      last.entries.push(entry);
    } else {
      groups.push({ key, label: valid ? dayLabel(date, now) : "Earlier", entries: [entry] });
    }
  }
  return groups;
}

/** The filter's `source` value for a source: `oxygen`, or the app's id. */
export function sourceKey(source: SourceNode): string {
  return source.app?.id ?? "oxygen";
}

/** A source's name in test ids: `oxygen`, or the app's slug. */
export function sourceTestKey(source: SourceNode): string {
  return source.app?.slug ?? "oxygen";
}

function findCategory(nodes: CategoryNode[], path: string): CategoryNode | undefined {
  for (const node of nodes) {
    if (node.path === path) return node;
    const inner = findCategory(node.categories, path);
    if (inner) return inner;
  }
  return undefined;
}

function findKind(nodes: CategoryNode[], kind: string): KindNode | undefined {
  for (const node of nodes) {
    const own = node.kinds.find((k) => k.kind === kind);
    if (own) return own;
    const inner = findKind(node.categories, kind);
    if (inner) return inner;
  }
  return undefined;
}

/**
 * The stream `kind` belongs to, as the catalogue places it — `undefined` for a
 * kind it does not name, and for a server that predates streams.
 *
 * Used to pick the open tab for a link that names a kind but no stream, so a
 * clicked push for a failed run opens on Operations with its branch of the tree
 * beside it, rather than on Activity, which never holds that kind.
 */
export function kindStream(
  catalog: NotificationCatalog | undefined,
  kind: string | undefined
): NotificationStream | undefined {
  if (!kind) return undefined;
  for (const source of catalog?.sources ?? []) {
    const found = findKind(source.categories, kind);
    if (found) return found.stream;
  }
  return undefined;
}

/** What the filter narrows to, in words: "All notifications", "Store Ops · Food safety". */
export function scopeLabel(catalog: NotificationCatalog | undefined, filter: InboxFilter): string {
  if (!filter.source) return "All notifications";
  const source = catalog?.sources.find((s) => sourceKey(s) === filter.source);
  if (!source) return filter.source === "oxygen" ? "Oxygen" : "Notifications";
  const leaf = filter.kind
    ? findKind(source.categories, filter.kind)?.label
    : filter.category
      ? findCategory(source.categories, filter.category)?.label
      : undefined;
  return leaf ? `${source.label} · ${leaf}` : source.label;
}

/**
 * `sources` with only what `stream` holds: its kinds, the categories that still
 * have one, and the sources that still have a category. `undefined` leaves the
 * tree alone, which is what a non-operator and an older server both get.
 *
 * A kind with no `stream` — a server that predates them — matches every stream,
 * so an old server's tree is never emptied by a filter it does not know about.
 *
 * A source's own count is re-summed from what survived; a category's is left as
 * the server computed it, because a category's kinds are all in one stream (an
 * audience is declared per kind, but Oxygen's categories group kinds that share
 * one, and every app kind is Activity).
 */
export function sourcesInStream(
  sources: SourceNode[],
  stream: NotificationStream | undefined
): SourceNode[] {
  if (!stream) return sources;
  const keep = (nodes: CategoryNode[]): CategoryNode[] =>
    nodes
      .map((node) => ({
        ...node,
        kinds: node.kinds.filter((k) => !k.stream || k.stream === stream),
        categories: keep(node.categories)
      }))
      .filter((node) => node.kinds.length > 0 || node.categories.length > 0);
  return sources
    .map((source) => {
      const categories = keep(source.categories);
      return {
        ...source,
        categories,
        unread: categories.reduce((sum, c) => sum + c.unread, 0)
      };
    })
    .filter((source) => source.categories.length > 0);
}
