import { ApiKeyService } from "@/services/api/apiKey";

/** "Never", "Today", "3 days ago", then the date once it is over a week old. */
export const formatLastUsed = (lastUsedAt?: string | null): string => {
  if (!lastUsedAt) return "Never";
  const diffMs = Date.now() - new Date(lastUsedAt).getTime();
  const diffDays = Math.floor(diffMs / (1000 * 60 * 60 * 24));

  if (diffDays === 0) return "Today";
  if (diffDays === 1) return "Yesterday";
  if (diffDays < 7) return `${diffDays} days ago`;

  return ApiKeyService.formatDate(lastUsedAt);
};
