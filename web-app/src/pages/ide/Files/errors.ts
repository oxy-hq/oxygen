import { isAxiosError } from "axios";

/**
 * What to tell the reader about a failed read. A `503` means the workspace has
 * nothing compiled yet (or the boundary could not be read): retryable, and
 * not the same as a workspace with no files.
 */
export function apiErrorMessage(error: unknown): string {
  if (isAxiosError(error)) {
    if (error.response?.status === 503) {
      return "Nothing is compiled for this workspace yet. Push to the default branch, then retry.";
    }
    if (error.response?.status === 404) return "The served revision has no such file.";
    const body = error.response?.data;
    if (typeof body === "string" && body.trim()) return body;
  }
  return error instanceof Error ? error.message : "Something went wrong.";
}
