import type {
  ApiKey,
  ApiKeyActivityResponse,
  ApiKeyListResponse,
  ExtendApiKeyRequest
} from "@/types/apiKey";
import { apiClient } from "./axios";

/**
 * The legacy `/{workspaceId}/api-keys` routes: legacy API keys only. There is no create here on
 * purpose. The endpoint still mints a legacy key for scripts, but the app creates API tokens.
 * The date helpers below are shared with the token surfaces.
 */
export class ApiKeyService {
  static async listApiKeys(projectId: string): Promise<ApiKeyListResponse> {
    const response = await apiClient.get<ApiKeyListResponse>(`/${projectId}/api-keys`);
    return response.data;
  }

  static async revokeApiKey(projectId: string, id: string): Promise<void> {
    await apiClient.delete(`/${projectId}/api-keys/${id}`);
  }

  /** Push out a key's expiry. The secret is unchanged; an expired (not revoked) key revives. */
  static async extendApiKey(
    projectId: string,
    id: string,
    request: ExtendApiKeyRequest
  ): Promise<ApiKey> {
    const response = await apiClient.post<ApiKey>(`/${projectId}/api-keys/${id}/extend`, request);
    return response.data;
  }

  static async getApiKeyActivity(
    projectId: string,
    id: string,
    limit = 100
  ): Promise<ApiKeyActivityResponse> {
    const response = await apiClient.get<ApiKeyActivityResponse>(
      `/${projectId}/api-keys/${id}/activity`,
      { params: { limit } }
    );
    return response.data;
  }

  static formatDate(dateString: string): string {
    return new Date(dateString).toLocaleDateString("en-US", {
      year: "numeric",
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit"
    });
  }

  /** A calendar day with no time, e.g. "Jan 30, 2027". */
  static formatDay(date: string | Date): string {
    return new Date(date).toLocaleDateString("en-US", {
      year: "numeric",
      month: "short",
      day: "numeric"
    });
  }

  // `null` is accepted because the server sends `expires_at: null` for a key with no expiry.
  static isExpired(expiresAt?: string | null): boolean {
    if (!expiresAt) return false;
    return new Date(expiresAt) < new Date();
  }

  /**
   * The expiry `POST …/extend` with `{ days }` produces: `days` counted from the later of now and
   * the current expiry, so extending a lapsed key counts from today, not from when it lapsed.
   */
  static extendedExpiry(
    currentExpiresAt: string | null | undefined,
    days: number,
    now = new Date()
  ) {
    const current = currentExpiresAt ? new Date(currentExpiresAt).getTime() : Number.NaN;
    const base = Number.isNaN(current) ? now.getTime() : Math.max(now.getTime(), current);
    return new Date(base + days * 24 * 60 * 60 * 1000);
  }

  static getTimeUntilExpiration(expiresAt?: string): string | null {
    if (!expiresAt) return null;

    const expirationDate = new Date(expiresAt);
    const now = new Date();
    const diffMs = expirationDate.getTime() - now.getTime();

    if (diffMs <= 0) return "Expired";

    const days = Math.floor(diffMs / (1000 * 60 * 60 * 24));
    if (days > 0) return `${days} day${days === 1 ? "" : "s"}`;

    const hours = Math.floor(diffMs / (1000 * 60 * 60));
    if (hours > 0) return `${hours} hour${hours === 1 ? "" : "s"}`;

    const minutes = Math.floor(diffMs / (1000 * 60));
    return `${minutes} minute${minutes === 1 ? "" : "s"}`;
  }
}
