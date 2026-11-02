import type {
  NodeSettings,
  NotificationChannel,
  NotificationLevel,
  SettingsCategory,
  SettingsKind,
  SettingsSource
} from "@/types/notifications";

/** The channels a person can turn off, in the order the switches show them. */
export const CHANNELS: { value: NotificationChannel; label: string }[] = [
  { value: "push", label: "Push" },
  { value: "email", label: "Email" },
  { value: "slack", label: "Slack" }
];

const LEVEL_RANK: Record<NotificationLevel, number> = {
  passive: 0,
  active: 1,
  time_sensitive: 2,
  critical: 3
};

/** The floors a person picks from, on a node or a device. `passive` lets everything through. */
export const FLOORS: { value: NotificationLevel; label: string }[] = [
  { value: "passive", label: "Everything" },
  { value: "time_sensitive", label: "Time-sensitive and above" },
  { value: "critical", label: "Critical only" }
];

/** A floor in words; `active` is not offered here but a phone may have set it. */
function floorLabel(level: NotificationLevel): string {
  if (level === "active") return "Active and above";
  return FLOORS.find((f) => f.value === level)?.label ?? "Everything";
}

/** What a row writes: a whole source or a category (`scope`), or one kind. */
type PrefTarget = { scope: string } | { kind: string };

/** One channel of one row, as the switch shows it. */
interface ChannelState {
  on: boolean;
  /** Set at this row itself, so it can be reset to inherit. */
  explicit: boolean;
  /** The row above whose setting this one inherits; `null` when explicit or the default (on). */
  from: string | null;
}

/** One row of a source's settings: the source itself, a category, or a kind. */
export interface PrefRow {
  /** Unique within the source; names the row's test ids. */
  key: string;
  label: string;
  description: string | null;
  /** 0 for the source, 1 for a top-level category, and so on. */
  depth: number;
  target: PrefTarget;
  /** The channels anything under this row can be delivered on: the only switches drawn. */
  channels: NotificationChannel[];
  /** A critical kind, or a subtree of nothing but critical kinds: locked on. */
  critical: boolean;
  state: Partial<Record<NotificationChannel, ChannelState>>;
  /** The floor set at this row itself. */
  floor: NotificationLevel | null;
  /** The row directly above, for "Same as …"; `null` on the source. */
  parent: string | null;
  /**
   * A kind under a floor above its level: inbox only, whatever its switches say.
   * `from` names the row that set the floor, `null` when the kind set it itself.
   */
  inboxOnly: { from: string | null } | null;
}

interface Ancestor {
  label: string;
  settings: NodeSettings;
}

function kindsUnder(node: SettingsCategory): SettingsKind[] {
  return [...node.kinds, ...node.categories.flatMap(kindsUnder)];
}

/** The union of `kinds`' channels, in switch order. */
function channelsOf(kinds: SettingsKind[]): NotificationChannel[] {
  const all = new Set(kinds.flatMap((k) => k.channels));
  return CHANNELS.map((c) => c.value).filter((c) => all.has(c));
}

function allCritical(kinds: SettingsKind[]): boolean {
  return kinds.length > 0 && kinds.every((k) => k.critical);
}

/** The nearest explicit setting wins: this row's, then each row above, nearest first; else on. */
function channelState(
  channel: NotificationChannel,
  own: NodeSettings,
  ancestors: Ancestor[]
): ChannelState {
  const mine = own.channels[channel];
  if (mine !== undefined) return { on: mine, explicit: true, from: null };
  for (let i = ancestors.length - 1; i >= 0; i--) {
    const set = ancestors[i].settings.channels[channel];
    if (set !== undefined) return { on: set, explicit: false, from: ancestors[i].label };
  }
  return { on: true, explicit: false, from: null };
}

function states(
  channels: NotificationChannel[],
  own: NodeSettings,
  ancestors: Ancestor[]
): PrefRow["state"] {
  return Object.fromEntries(channels.map((c) => [c, channelState(c, own, ancestors)]));
}

/** Whether `kind` is below the nearest floor, and which row set it. */
function inboxOnly(kind: SettingsKind, ancestors: Ancestor[]): PrefRow["inboxOnly"] {
  if (kind.critical) return null;
  let floor = kind.settings.min_level;
  let from: string | null = null;
  for (let i = ancestors.length - 1; floor === null && i >= 0; i--) {
    floor = ancestors[i].settings.min_level;
    from = ancestors[i].label;
  }
  return floor && LEVEL_RANK[kind.level] < LEVEL_RANK[floor] ? { from } : null;
}

function kindRow(kind: SettingsKind, depth: number, ancestors: Ancestor[]): PrefRow {
  return {
    key: `kind-${kind.kind}`,
    label: kind.label,
    description: kind.description,
    depth,
    target: { kind: kind.kind },
    channels: kind.channels,
    critical: kind.critical,
    state: states(kind.channels, kind.settings, ancestors),
    floor: kind.settings.min_level,
    parent: ancestors[ancestors.length - 1]?.label ?? null,
    inboxOnly: inboxOnly(kind, ancestors)
  };
}

function categoryRows(node: SettingsCategory, depth: number, ancestors: Ancestor[]): PrefRow[] {
  const kinds = kindsUnder(node);
  const channels = channelsOf(kinds);
  const row: PrefRow = {
    key: `cat-${node.path}`,
    label: node.label,
    description: node.description,
    depth,
    target: { scope: node.path },
    channels,
    critical: allCritical(kinds),
    state: states(channels, node.settings, ancestors),
    floor: node.settings.min_level,
    parent: ancestors[ancestors.length - 1]?.label ?? null,
    inboxOnly: null
  };
  const below = [...ancestors, { label: node.label, settings: node.settings }];
  return [
    row,
    ...node.kinds.map((kind) => kindRow(kind, depth + 1, below)),
    ...node.categories.flatMap((child) => categoryRows(child, depth + 1, below))
  ];
}

/**
 * A source's tree as the rows Settings draws, top down: the source, then each
 * category followed by its kinds and its own categories. Every channel of
 * every row is resolved the way delivery resolves it — the nearest explicit
 * setting wins — so a row shows what it inherits, and from where.
 */
export function prefRows(source: SettingsSource): PrefRow[] {
  const kinds = source.categories.flatMap(kindsUnder);
  const channels = channelsOf(kinds);
  const top: PrefRow = {
    key: "source",
    label: source.label,
    description: null,
    depth: 0,
    target: { scope: "" },
    channels,
    critical: allCritical(kinds),
    state: states(channels, source.settings, []),
    floor: source.settings.min_level,
    parent: null,
    inboxOnly: null
  };
  const ancestors = [{ label: source.label, settings: source.settings }];
  return [top, ...source.categories.flatMap((c) => categoryRows(c, 1, ancestors))];
}

const channelName = (c: NotificationChannel) => CHANNELS.find((x) => x.value === c)?.label ?? c;

/**
 * "From Store operations" when every inherited switch of the row takes its
 * value from that row; otherwise each group named: "Email from Store
 * operations · Push from Oxygen". `null` when nothing is inherited from a row.
 */
export function inheritedNote(row: PrefRow): string | null {
  const inherited = row.channels.filter((c) => !row.state[c]?.explicit);
  const byRow = new Map<string, NotificationChannel[]>();
  for (const c of inherited) {
    const from = row.state[c]?.from;
    if (from) byRow.set(from, [...(byRow.get(from) ?? []), c]);
  }
  if (byRow.size === 0) return null;
  const groups = [...byRow];
  if (groups.length === 1 && groups[0][1].length === inherited.length)
    return `From ${groups[0][0]}`;
  return groups
    .map(([from, channels]) => `${channels.map(channelName).join(", ")} from ${from}`)
    .join(" · ");
}

/** What a row says under its label, or `null`. */
export function rowNote(row: PrefRow): string | null {
  if (row.critical) return "Critical — always delivered";
  if (row.inboxOnly) {
    return row.inboxOnly.from ? `Inbox only — floor on ${row.inboxOnly.from}` : "Inbox only";
  }
  const parts = [
    row.floor && row.floor !== "passive" ? floorLabel(row.floor) : null,
    inheritedNote(row)
  ].filter((p): p is string => p !== null);
  return parts.length > 0 ? parts.join(" · ") : null;
}
