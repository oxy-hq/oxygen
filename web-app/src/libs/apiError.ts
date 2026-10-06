import { isAxiosError } from "axios";

/**
 * The message an org route put in its error body. The crew and operating
 * graph routes answer `{ error }`; older org routes answer `{ message }`.
 * Neither is guaranteed, so the caller names a fallback.
 */
export function apiErrorMessage(err: unknown, fallback: string): string {
  if (isAxiosError(err)) {
    const data: unknown = err.response?.data;
    if (data && typeof data === "object") {
      const body = data as { error?: unknown; message?: unknown };
      if (typeof body.error === "string" && body.error) return body.error;
      if (typeof body.message === "string" && body.message) return body.message;
    }
    return fallback;
  }
  return err instanceof Error && err.message ? err.message : fallback;
}

/**
 * The machine-readable `code` a route put beside `error`, if any. The server's sentence is
 * written for a log; the code is what the UI keys its own, more useful sentence on.
 */
export function apiErrorCode(err: unknown): string | undefined {
  if (!isAxiosError(err)) return undefined;
  const data: unknown = err.response?.data;
  if (!data || typeof data !== "object") return undefined;
  const code = (data as { code?: unknown }).code;
  return typeof code === "string" && code ? code : undefined;
}

export function apiStatus(err: unknown): number | undefined {
  return isAxiosError(err) ? err.response?.status : undefined;
}
