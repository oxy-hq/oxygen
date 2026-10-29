/** A channel a notification can reach someone on, besides the inbox itself. */
export type NotificationChannel = "push" | "email" | "slack";

/**
 * How loudly a kind interrupts. Only `critical` ignores mutes; mobile maps the
 * rest to iOS interruption levels and Android importance.
 */
export type NotificationLevel = "passive" | "active" | "time_sensitive" | "critical";

/** The custom app a notification came from. */
export interface NotificationApp {
  id: string;
  slug: string;
  name: string;
}

/**
 * A button a kind offers. No function: "open", which follows the notification's
 * link. With one (`runs_function`), the server calls the app's function as the
 * signed-in person.
 */
export interface NotificationAction {
  id: string;
  label: string;
  destructive: boolean;
  runs_function: boolean;
}

/** Where a notification sits in its source's tree. */
export interface NotificationCategoryRef {
  /** `operations/food-safety`; `general` for a kind with no category. */
  path: string;
  /** Root → leaf, e.g. `["Store operations", "Food safety"]`. */
  labels: string[];
}

/** One row of the inbox (`GET /notifications`). */
export interface InboxItem {
  id: string;
  org_id: string;
  kind: string;
  created_at: string;
  read_at: string | null;
  /** `null` when the org's store could not be read right now. */
  title: string | null;
  body: string | null;
  /**
   * Relative URL. An app's lands in the app (`/customer-apps/<org>/<app>/…`); a
   * platform notice's is a path inside this SPA.
   */
  href: string | null;
  /**
   * The path exactly as the app passed it to `ctx.notify` (e.g. `/bookings/1`),
   * for the app's own router. `null` for a platform notice. Oxygen follows `href`.
   */
  link: string | null;
  /** `null` for a platform notice, whose `kind` starts with `oxy.`. */
  app: NotificationApp | null;
  // The fields below are absent from a server older than notification
  // categories; every reader treats a missing one as "nothing to add".
  category?: NotificationCategoryRef;
  kind_label?: string;
  level?: NotificationLevel;
  /** Opaque: items sharing one collapse into the newest. */
  thread?: string | null;
  actions?: NotificationAction[];
  /** Absolute deep link, for mobile and sharing. Oxygen follows `href`. */
  url?: string | null;
}

export interface InboxResponse {
  unread: number;
  items: InboxItem[];
  /** The cursor for the next (older) page; absent or `null` when there is none. */
  next_before?: string | null;
}

/**
 * What the inbox list is narrowed to. `source` is `oxygen` or an app id;
 * `category` (a subtree) needs `source`.
 */
export interface InboxFilter {
  source?: string;
  category?: string;
  kind?: string;
  thread?: string;
}

/** The answer to running a notification's action. */
export interface NotificationActionResult {
  ok: boolean;
  /** A function action's JSON answer, or `null`. */
  result?: unknown;
  /** An open action's destination. */
  url?: string;
}

/** A kind, the leaf of the catalogue tree. */
export interface KindNode {
  kind: string;
  label: string;
  description: string | null;
  level: NotificationLevel;
  /** `level === "critical"`: delivered whatever is muted. */
  critical: boolean;
  /** The channels this kind can be delivered on. */
  channels: NotificationChannel[];
  /** Mutes set at the kind itself. */
  muted: NotificationChannel[];
  actions: NotificationAction[];
  unread: number;
}

/** A category: a path segment of a source's tree. */
export interface CategoryNode {
  path: string;
  label: string;
  description: string | null;
  /** Mutes set at this node (a parent's are not repeated here). */
  muted: NotificationChannel[];
  unread: number;
  categories: CategoryNode[];
  kinds: KindNode[];
}

/** Oxygen, or one custom app: the root of a tree. */
export interface SourceNode {
  source: "oxygen" | "app";
  app: (NotificationApp & { org_slug: string }) | null;
  label: string;
  /** Mutes set on the whole source. */
  muted: NotificationChannel[];
  unread: number;
  categories: CategoryNode[];
}

/** `GET /notifications/catalog`: every source that can tell this person something. */
export interface NotificationCatalog {
  sources: SourceNode[];
  reach: NotificationReach;
}

export interface UnreadResponse {
  unread: number;
  /** The apps with something unread. Absent from a server that predates it. */
  apps?: AppUnread[];
}

export interface AppUnread {
  app_id: string;
  unread: number;
}

export interface NotificationReach {
  push_devices: number;
  email: boolean;
}

export interface VapidKeyResponse {
  configured: boolean;
  /** base64url, uncompressed P-256 point. */
  public_key: string | null;
}

export interface RegisterDeviceInput {
  platform: "web";
  /** For web, the `PushSubscription` endpoint URL. */
  token: string;
  device_name?: string;
  /**
   * The subscription's encryption keys (`PushSubscription.toJSON().keys`). With
   * them a push names its app and kind; without, it reads "You have a new
   * notification".
   */
  keys?: { p256dh: string; auth: string };
}

// ── Settings (`GET/PATCH /notifications/settings`) ────────────────────────────
//
// Four levels: the person (pause, quiet hours), each node of a source's tree,
// each device, and `critical`, which overrides the first two. The nearest
// explicit setting wins, so a child can turn a channel back on under a parent
// that turned it off.

/** Why a kind is, or is not, delivered on a channel (the server's `decide`). */
export type NotificationReason =
  | null
  | "paused"
  | "quiet_hours"
  | "muted"
  | "below_level"
  | "device_off"
  | "os_disabled"
  | "os_denied"
  | "not_sent";

export interface QuietHours {
  /** `HH:MM`, in the person's `timezone`. A window may wrap past midnight. */
  start: string;
  end: string;
  /** 0 = Sunday. */
  days: number[];
  /** The lowest level that still comes through. */
  allow: "critical" | "time_sensitive";
}

export interface GlobalNotificationSettings {
  /** ISO instant; push, email and Slack hold everything but critical until then. */
  paused_until: string | null;
  quiet_hours: QuietHours | null;
  /** IANA; required once quiet hours are set. */
  timezone: string | null;
}

/** What is set at one node itself; anything absent inherits. */
export interface NodeSettings {
  channels: Partial<Record<NotificationChannel, boolean>>;
  /** Kinds below this go to the inbox only. */
  min_level: NotificationLevel | null;
}

export interface EffectiveChannel {
  on: boolean;
  reason: NotificationReason;
  /** The label of the node that decided it. */
  from: string | null;
}

export interface SettingsKind {
  kind: string;
  label: string;
  description: string | null;
  level: NotificationLevel;
  critical: boolean;
  /** The channels this kind can ever use. */
  channels: NotificationChannel[];
  settings: NodeSettings;
  effective: Record<NotificationChannel, EffectiveChannel>;
  unread: number;
}

export interface SettingsCategory {
  path: string;
  label: string;
  description: string | null;
  settings: NodeSettings;
  categories: SettingsCategory[];
  kinds: SettingsKind[];
  unread: number;
}

export interface SettingsSource {
  source: "oxygen" | "app";
  app: (NotificationApp & { org_slug: string }) | null;
  label: string;
  settings: NodeSettings;
  categories: SettingsCategory[];
  unread: number;
}

export type DevicePlatform = "web" | "ios" | "android";

export type OsPermission = "granted" | "denied" | "provisional" | "ephemeral" | "unknown";

/** One registered push target: a browser, a phone, a watch. */
export interface NotificationDevice {
  id: string;
  platform: DevicePlatform;
  name: string | null;
  app_version: string | null;
  created_at: string;
  last_seen_at: string;
  /** Off stops push here without unregistering it. */
  enabled: boolean;
  /** Push here only at this level and above; `null` for everything. */
  min_level: NotificationLevel | null;
  /** As the device last reported it; `null` before it has. */
  os_permission: OsPermission | null;
  /** `<app id|oxygen>.<kind>` ids the OS has switched off. */
  os_disabled: string[];
}

export interface SettingsReach {
  push_devices: number;
  email: boolean;
  slack: boolean;
}

/** `GET /notifications/settings`. Served with `ETag: <version>`. */
export interface NotificationSettings {
  /** Opaque; changes on every write. Sent back as `If-Match`. */
  version: string;
  global: GlobalNotificationSettings;
  /** The same sources as the catalogue. */
  sources: SettingsSource[];
  devices: NotificationDevice[];
  reach: SettingsReach;
}

/**
 * One node to change: a whole source (`scope: ""`), a category (`scope:
 * "<path>"`) or a kind — exactly one of `scope` / `kind`. A channel set to
 * `null`, or `min_level: null`, goes back to inheriting.
 */
export interface NodePatch {
  /** `null` for Oxygen. */
  app_id: string | null;
  scope?: string;
  kind?: string;
  channels?: Partial<Record<NotificationChannel, boolean | null>>;
  min_level?: NotificationLevel | null;
}

/** `PATCH /notifications/settings`: one transaction. */
export interface NotificationSettingsPatch {
  global?: Partial<GlobalNotificationSettings>;
  nodes?: NodePatch[];
}

/** `PATCH /notifications/devices/{id}`. */
export interface DevicePatch {
  name?: string;
  enabled?: boolean;
  min_level?: NotificationLevel | null;
  os_permission?: OsPermission;
  os_disabled?: string[];
  app_version?: string;
}
