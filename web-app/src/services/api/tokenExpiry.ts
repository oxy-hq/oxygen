/**
 * Dates and expiry, as every credential surface reads them: the personal-token
 * list, an org's service accounts, the staff token pages and the Extend
 * popover.
 *
 * This was `ApiKeyService`, which also held the legacy `/{workspaceId}/api-keys`
 * calls. Those routes went with legacy API keys; the helpers were always shared
 * with the token surfaces, so they are all that is left — and they talk to
 * nothing, which is why there is no client here.
 */
export class TokenExpiry {
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

  // `null` is accepted because the server sends `expires_at: null` for a token with no expiry.
  static isExpired(expiresAt?: string | null): boolean {
    if (!expiresAt) return false;
    return new Date(expiresAt) < new Date();
  }

  /**
   * The expiry `POST …/extend` with `{ days }` produces: `days` counted from the later of now and
   * the current expiry, so extending a lapsed token counts from today, not from when it lapsed.
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
