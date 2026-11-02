import { isAxiosError } from "axios";

/** The HTTP status a failed request answered with, when it got that far. */
function httpStatus(error: unknown): number | undefined {
  return isAxiosError(error) ? error.response?.status : undefined;
}

/** A 403: the caller may not do this. An answer to show, not a fault to retry. */
export function isForbidden(error: unknown): boolean {
  return httpStatus(error) === 403;
}
