import type {
  DevicePatch,
  InboxFilter,
  InboxItem,
  InboxResponse,
  NotificationActionResult,
  NotificationCatalog,
  NotificationDevice,
  NotificationSettings,
  NotificationSettingsPatch,
  RegisterDeviceInput,
  UnreadResponse,
  VapidKeyResponse
} from "@/types/notifications";
import { apiClient } from "./axios";

/** A page of the inbox: newest first, `before` an item id for the page after it. */
export interface InboxPageQuery extends InboxFilter {
  unreadOnly: boolean;
  limit: number;
  before?: string;
}

/** The filter as query params: only the keys that are set. */
function filterParams(filter: InboxFilter): Record<string, string> {
  const params: Record<string, string> = {};
  if (filter.source) {
    params.source = filter.source;
    // An app id is also `app_id`, which every server narrows by.
    if (filter.source !== "oxygen") params.app_id = filter.source;
  }
  if (filter.category) params.category = filter.category;
  if (filter.kind) params.kind = filter.kind;
  if (filter.thread) params.thread = filter.thread;
  return params;
}

/**
 * The signed-in person's notifications: the inbox, their settings, and the
 * devices push reaches. Every route is the caller's own; none takes a workspace.
 */
export class NotificationsService {
  /** Newest first. `unreadOnly` defaults to `true` server-side, so it is always sent. */
  static async inbox(params: InboxPageQuery): Promise<InboxResponse> {
    const { unreadOnly, limit, before, ...filter } = params;
    const response = await apiClient.get<InboxResponse>("/notifications", {
      params: {
        unread_only: unreadOnly,
        limit,
        ...filterParams(filter),
        ...(before ? { before } : {})
      }
    });
    return response.data;
  }

  /** One notification of mine. */
  static async item(id: string): Promise<InboxItem> {
    const response = await apiClient.get<InboxItem>(`/notifications/${encodeURIComponent(id)}`);
    return response.data;
  }

  /** The badge count. Cheap: it never reads an org's store. */
  static async unread(): Promise<UnreadResponse> {
    const response = await apiClient.get<UnreadResponse>("/notifications/unread");
    return response.data;
  }

  /**
   * Sent with `keepalive`: clicking a notification marks it read and then
   * follows its link, which for a custom app leaves this page. Without it the
   * browser may cancel the request as the page unloads, and the item comes back
   * unread.
   */
  static async markRead(id: string): Promise<void> {
    await apiClient.post(`/notifications/${encodeURIComponent(id)}/read`, undefined, {
      adapter: "fetch",
      fetchOptions: { keepalive: true }
    });
  }

  /** Everything read, or everything the filter covers. */
  static async markAllRead(filter: InboxFilter = {}): Promise<void> {
    await apiClient.post("/notifications/read-all", undefined, { params: filterParams(filter) });
  }

  /** Run one of a notification's actions as me. It also marks the notification read. */
  static async runAction(id: string, actionId: string): Promise<NotificationActionResult> {
    const response = await apiClient.post<NotificationActionResult>(
      `/notifications/${encodeURIComponent(id)}/actions/${encodeURIComponent(actionId)}`
    );
    return response.data;
  }

  /** Sources → categories → kinds, with my mutes and unread counts at every node. */
  static async catalog(): Promise<NotificationCatalog> {
    const response = await apiClient.get<NotificationCatalog>("/notifications/catalog");
    return response.data;
  }

  /** Pause, quiet hours, every node's own settings, devices and reach, as one document. */
  static async settings(): Promise<NotificationSettings> {
    const response = await apiClient.get<NotificationSettings>("/notifications/settings");
    return response.data;
  }

  /**
   * One transaction; answers with the whole new document. With `version`, a
   * document changed since then is refused with 412 and the current one.
   */
  static async updateSettings(
    patch: NotificationSettingsPatch,
    version?: string
  ): Promise<NotificationSettings> {
    const response = await apiClient.patch<NotificationSettings>("/notifications/settings", patch, {
      headers: version ? { "If-Match": version } : undefined
    });
    return response.data;
  }

  static async devices(): Promise<NotificationDevice[]> {
    const response = await apiClient.get<NotificationDevice[]>("/notifications/devices");
    return response.data;
  }

  static async updateDevice(id: string, patch: DevicePatch): Promise<void> {
    await apiClient.patch(`/notifications/devices/${encodeURIComponent(id)}`, patch);
  }

  static async removeDevice(id: string): Promise<void> {
    await apiClient.delete(`/notifications/devices/${encodeURIComponent(id)}`);
  }

  static async vapidPublicKey(): Promise<VapidKeyResponse> {
    const response = await apiClient.get<VapidKeyResponse>("/notifications/vapid-public-key");
    return response.data;
  }

  /** The registered device's id, or `null` from a server that does not answer with one. */
  static async registerDevice(input: RegisterDeviceInput): Promise<string | null> {
    const response = await apiClient.post<{ id?: unknown } | "">("/notifications/devices", input);
    const id = typeof response.data === "object" ? response.data?.id : undefined;
    return typeof id === "string" ? id : null;
  }
}
