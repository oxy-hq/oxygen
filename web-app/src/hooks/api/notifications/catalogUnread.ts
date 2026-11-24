import type { QueryClient } from "@tanstack/react-query";
import type {
  CategoryNode,
  InboxFilter,
  InboxItem,
  KindNode,
  NotificationCatalog,
  SourceNode
} from "@/types/notifications";
import queryKeys from "../queryKey";

/**
 * The catalogue's unread counts, kept in step with a read here rather than
 * re-read from the server: the catalogue is the costly notifications read, and a
 * click on a row is the commonest thing on the page. It refreshes on its own
 * staleness and on focus, and a preference write re-reads it.
 */

/** What to take off where, inside one source. */
interface Take {
  /** A category whose whole subtree is cleared. */
  category?: (node: CategoryNode) => boolean;
  /** How much to take off a kind. */
  kind: (node: KindNode) => number;
}

/** A node with unread taken off, and how much came off. */
interface Taken<T> {
  node: T;
  taken: number;
}

function cleared(node: CategoryNode): Taken<CategoryNode> {
  const zero = (n: CategoryNode): CategoryNode => ({
    ...n,
    unread: 0,
    kinds: n.kinds.map((k) => ({ ...k, unread: 0 })),
    categories: n.categories.map(zero)
  });
  return { node: zero(node), taken: node.unread };
}

function takeFromCategories(categories: CategoryNode[], take: Take): Taken<CategoryNode[]> {
  let taken = 0;
  const nodes = categories.map((c) => {
    const t = take.category?.(c) ? cleared(c) : takeFromCategory(c, take);
    taken += t.taken;
    return t.node;
  });
  return { node: taken ? nodes : categories, taken };
}

function takeFromCategory(node: CategoryNode, take: Take): Taken<CategoryNode> {
  let taken = 0;
  const kinds = node.kinds.map((k) => {
    const off = Math.min(k.unread, take.kind(k));
    taken += off;
    return off > 0 ? { ...k, unread: k.unread - off } : k;
  });
  const below = takeFromCategories(node.categories, take);
  taken += below.taken;
  if (!taken) return { node, taken };
  return {
    node: { ...node, unread: Math.max(0, node.unread - taken), kinds, categories: below.node },
    taken
  };
}

const sourceKey = (source: SourceNode) => source.app?.id ?? "oxygen";

function inSource(
  catalog: NotificationCatalog,
  wanted: (source: SourceNode) => boolean,
  take: Take
): NotificationCatalog {
  return {
    ...catalog,
    sources: catalog.sources.map((source) => {
      if (!wanted(source)) return source;
      const { node: categories, taken } = takeFromCategories(source.categories, take);
      return taken ? { ...source, unread: Math.max(0, source.unread - taken), categories } : source;
    })
  };
}

/**
 * `catalog` once `item` is read: one off its source, every category on its path
 * and its kind. Unchanged for an item that was already read.
 */
export function withItemRead(
  catalog: NotificationCatalog,
  item: Pick<InboxItem, "app" | "kind" | "read_at">
): NotificationCatalog {
  if (item.read_at !== null) return catalog;
  // A kind sits in one place in its source's tree; one off the first found.
  let left = 1;
  return inSource(catalog, (s) => sourceKey(s) === (item.app?.id ?? "oxygen"), {
    kind: (k) => (k.kind === item.kind && left-- > 0 ? 1 : 0)
  });
}

/**
 * `catalog` once everything `filter` covers is read: every source for `{}`, one
 * source, a category's subtree, or a kind. `null` when the catalogue cannot say
 * what a filter covered (a thread, or one stream of a source), so it is re-read
 * instead.
 */
export function withFilterRead(
  catalog: NotificationCatalog,
  filter: InboxFilter
): NotificationCatalog | null {
  const { source, category, kind, thread, stream } = filter;
  // A stream cuts across the tree — one of a source's categories is in it and
  // another is not — so zeroing a whole source would take the other tab's
  // counts down with it. Re-read rather than guess.
  if (thread || stream || (category && (!source || kind))) return null;
  const wanted = (s: SourceNode) => !source || sourceKey(s) === source;
  if (category) {
    return inSource(catalog, wanted, { category: (c) => c.path === category, kind: () => 0 });
  }
  if (kind) {
    return inSource(catalog, wanted, { kind: (k) => (k.kind === kind ? k.unread : 0) });
  }
  return {
    ...catalog,
    sources: catalog.sources.map((s) =>
      wanted(s) ? { ...s, unread: 0, categories: s.categories.map((c) => cleared(c).node) } : s
    )
  };
}

/**
 * Apply `update` to the cached catalogue, if there is one. A read already in
 * flight is cancelled first, so an answer from before this read cannot put the
 * count back. An update that cannot tell (`null`) re-reads it instead.
 */
export async function updateCatalogUnread(
  qc: QueryClient,
  update: (catalog: NotificationCatalog) => NotificationCatalog | null
) {
  const key = queryKeys.notifications.catalog();
  if (!qc.getQueryData<NotificationCatalog>(key)) return;
  await qc.cancelQueries({ queryKey: key });
  const current = qc.getQueryData<NotificationCatalog>(key);
  if (!current) return;
  const next = update(current);
  if (next) qc.setQueryData(key, next);
  else qc.invalidateQueries({ queryKey: key });
}

/** Re-read the catalogue: after a read failed, its counts may be off. */
export function refetchCatalog(qc: QueryClient) {
  qc.invalidateQueries({ queryKey: queryKeys.notifications.catalog() });
}
